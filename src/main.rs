//! Raspberry Pi Pico - "Dodge & Shoot!" Game
//!
//! Pico Display Pack buttons:
//!   B (GP13) = left, Y (GP15) = right, X (GP14) = fire
//! LED (GP25): ON during gameplay, OFF otherwise

#![no_std]
#![no_main]

use core::fmt::Write as _;
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::USB;
use embassy_rp::spi::{self, Spi};
use embassy_rp::usb::{Driver, InterruptHandler as UsbInterruptHandler};
use embassy_rp::bind_interrupts;
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::Builder;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// --- Screen ---
const SCREEN_W: i32 = 240;
const SCREEN_H: i32 = 135;

// --- Player ---
const PLAYER_W: i32 = 24;
const PLAYER_H: i32 = 8;
const PLAYER_Y: i32 = 120;
const PLAYER_SPEED: i32 = 5;

// --- Obstacles ---
const OBS_W: i32 = 12;
const OBS_H: i32 = 8;
const MAX_OBS: usize = 6;
const INITIAL_SPEED: i32 = 2;

// --- Missiles ---
const MISSILE_W: i32 = 3;
const MISSILE_H: i32 = 6;
const MISSILE_SPEED: i32 = 4;
const MAX_MISSILES: usize = 4;

// --- Particles (debris) ---
const MAX_PARTICLES: usize = 24;
const PARTICLE_LIFE: u8 = 8;

// --- Lives ---
const MAX_LIVES: u8 = 3;

// --- HUD ---
const HUD_H: i32 = 14;

// --- Game states ---
#[derive(PartialEq, Clone, Copy)]
enum GameState {
    Title,
    Playing,
    GameOver,
}

#[derive(Clone, Copy)]
struct Obstacle {
    x: i32,
    y: i32,
    active: bool,
}

impl Obstacle {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            active: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Missile {
    x: i32,
    y: i32,
    active: bool,
}

impl Missile {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            active: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Particle {
    x: i32,
    y: i32,
    dx: i32,
    dy: i32,
    life: u8,
}

impl Particle {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            dx: 0,
            dy: 0,
            life: 0,
        }
    }
}

// --- xorshift32 PRNG ---
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    fn range(&mut self, max: i32) -> i32 {
        (self.next_u32() % max as u32) as i32
    }
}

fn aabb_overlap(ax: i32, ay: i32, aw: i32, ah: i32, bx: i32, by: i32, bw: i32, bh: i32) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

// --- Embassy bindings ---
bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
});

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

// --- Main ---
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // USB serial logger
    let usb_driver = Driver::new(p.USB, Irqs);
    unwrap!(spawner.spawn(logger_task(usb_driver)));
    Timer::after(Duration::from_secs(2)).await;
    log::info!("=== Dodge & Shoot Game ===");

    // Onboard LED (GP25 on Pico)
    let mut led = Output::new(p.PIN_25, Level::Low);
    log::info!("LED ready!");

    // ST7789 display (Pico Display Pack)
    let _bl = Output::new(p.PIN_20, Level::High);
    let mut spi_config = spi::Config::default();
    spi_config.frequency = 62_500_000;
    let spi_bus = Spi::new_blocking_txonly(p.SPI0, p.PIN_18, p.PIN_19, spi_config);
    let cs_display = Output::new(p.PIN_17, Level::High);
    let dc = Output::new(p.PIN_16, Level::Low);
    let spi_device = ExclusiveDevice::new_no_delay(spi_bus, cs_display).unwrap();
    static DISPLAY_BUF: StaticCell<[u8; 1024]> = StaticCell::new();
    let display_buf = DISPLAY_BUF.init([0u8; 1024]);
    let di = mipidsi::interface::SpiInterface::new(spi_device, dc, display_buf);
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .init(&mut Delay)
        .unwrap();
    display.clear(Rgb565::BLACK).unwrap();
    log::info!("Display ready!");

    // Buttons (active-low, pull-up)
    //  [A]  [X]  ← X = fire
    //  [B]  [Y]  ← B = left, Y = right
    let btn_a = Input::new(p.PIN_12, Pull::Up); // Fire
    let btn_b = Input::new(p.PIN_13, Pull::Up); // Left
    let btn_x = Input::new(p.PIN_14, Pull::Up); // Fire
    let btn_y = Input::new(p.PIN_15, Pull::Up); // Right

    // Game variables
    let mut game_state = GameState::Title;
    let mut prev_state = GameState::Playing; // force initial title draw
    let mut player_x: i32 = (SCREEN_W - PLAYER_W) / 2;
    let mut obstacles = [Obstacle::new(); MAX_OBS];
    let mut missiles = [Missile::new(); MAX_MISSILES];
    let mut particles = [Particle::new(); MAX_PARTICLES];
    let mut score: u32 = 0;
    let mut lives: u8 = MAX_LIVES;
    let mut spawn_timer: u32 = 0;
    let mut rng = Rng::new(12345);
    let mut rng_seeded = false;
    let mut invincible: u32 = 0;
    let mut frame: u32 = 0;
    let mut prev_score: u32 = u32::MAX;
    let mut prev_lives: u8 = u8::MAX;
    let mut prev_a = false;
    let mut prev_b = false;
    let mut prev_x = false;
    let mut prev_y = false;
    let mut buf = heapless::String::<32>::new();

    // Text styles
    let title_style = MonoTextStyle::new(&FONT_10X20, Rgb565::YELLOW);
    let hud_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let gameover_style = MonoTextStyle::new(&FONT_10X20, Rgb565::RED);
    let info_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    // Colors
    let player_color = Rgb565::CYAN;
    let obs_color = Rgb565::RED;
    let missile_color = Rgb565::YELLOW;
    let life_on = Rgb565::RED;
    let life_off = Rgb565::new(4, 8, 4);

    log::info!("Entering game loop");

    loop {
        let frame_start = Instant::now();

        // Poll buttons
        let a_down = btn_a.is_low();
        let b_down = btn_b.is_low();
        let x_down = btn_x.is_low();
        let y_down = btn_y.is_low();
        let a_just = a_down && !prev_a;
        let b_just = b_down && !prev_b;
        let x_just = x_down && !prev_x;
        let y_just = y_down && !prev_y;
        prev_a = a_down;
        prev_b = b_down;
        prev_x = x_down;
        prev_y = y_down;

        // Seed RNG on first button press
        if !rng_seeded && (a_down || b_down || x_down || y_down) {
            rng = Rng::new(Instant::now().as_ticks() as u32);
            rng_seeded = true;
        }

        match game_state {
            // ==================== TITLE ====================
            GameState::Title => {
                if prev_state != GameState::Title {
                    display.clear(Rgb565::BLACK).unwrap();
                    Text::with_baseline(
                        "DODGE!",
                        Point::new(90, 20),
                        title_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    Text::with_baseline(
                        "B:Left Y:Right A/X:Fire",
                        Point::new(48, 60),
                        info_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    Text::with_baseline(
                        "Press any button",
                        Point::new(72, 85),
                        info_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    Text::with_baseline(
                        "to start",
                        Point::new(96, 100),
                        info_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    led.set_low();
                    prev_state = GameState::Title;
                    log::info!("Title screen");
                }

                if a_just || b_just || x_just || y_just {
                    // Reset game
                    player_x = (SCREEN_W - PLAYER_W) / 2;
                    for obs in obstacles.iter_mut() {
                        obs.active = false;
                    }
                    for m in missiles.iter_mut() {
                        m.active = false;
                    }
                    for p in particles.iter_mut() {
                        p.life = 0;
                    }
                    score = 0;
                    lives = MAX_LIVES;
                    spawn_timer = 0;
                    invincible = 0;
                    prev_score = u32::MAX;
                    prev_lives = u8::MAX;
                    game_state = GameState::Playing;
                    log::info!("Game start!");
                }
            }

            // ==================== PLAYING ====================
            GameState::Playing => {
                // First frame: clear screen, turn LED on
                if prev_state != GameState::Playing {
                    display.clear(Rgb565::BLACK).unwrap();
                    led.set_high();
                    prev_state = GameState::Playing;
                }

                // --- Input: movement ---
                if b_down {
                    player_x = (player_x - PLAYER_SPEED).max(0);
                }
                if y_down {
                    player_x = (player_x + PLAYER_SPEED).min(SCREEN_W - PLAYER_W);
                }

                // --- Input: fire missile (A or X) ---
                if a_just || x_just {
                    for m in missiles.iter_mut() {
                        if !m.active {
                            m.x = player_x + PLAYER_W / 2 - MISSILE_W / 2;
                            m.y = PLAYER_Y - MISSILE_H;
                            m.active = true;
                            break;
                        }
                    }
                }

                // --- Obstacle speed (increases every 10 points) ---
                let speed = (INITIAL_SPEED + (score / 10) as i32).min(6);

                // --- Spawn obstacles ---
                spawn_timer += 1;
                let spawn_interval = 30u32.saturating_sub((score / 10) * 5).max(10);
                if spawn_timer >= spawn_interval {
                    spawn_timer = 0;
                    for obs in obstacles.iter_mut() {
                        if !obs.active {
                            obs.x = rng.range(SCREEN_W - OBS_W);
                            obs.y = HUD_H;
                            obs.active = true;
                            break;
                        }
                    }
                }

                // --- Move obstacles ---
                for obs in obstacles.iter_mut() {
                    if !obs.active {
                        continue;
                    }
                    obs.y += speed;
                    if obs.y > SCREEN_H {
                        obs.active = false;
                        score += 1;
                    }
                }

                // --- Move missiles ---
                for m in missiles.iter_mut() {
                    if !m.active {
                        continue;
                    }
                    m.y -= MISSILE_SPEED;
                    if m.y + MISSILE_H < HUD_H {
                        m.active = false;
                    }
                }

                // --- Update particles ---
                for p in particles.iter_mut() {
                    if p.life == 0 {
                        continue;
                    }
                    p.x += p.dx;
                    p.y += p.dy;
                    p.life -= 1;
                }

                // --- Missile-obstacle collision ---
                for mi in 0..MAX_MISSILES {
                    if !missiles[mi].active {
                        continue;
                    }
                    for oi in 0..MAX_OBS {
                        if !obstacles[oi].active {
                            continue;
                        }
                        if aabb_overlap(
                            missiles[mi].x,
                            missiles[mi].y,
                            MISSILE_W,
                            MISSILE_H,
                            obstacles[oi].x,
                            obstacles[oi].y,
                            OBS_W,
                            OBS_H,
                        ) {
                            // Spawn debris particles at obstacle center
                            let cx = obstacles[oi].x + OBS_W / 2;
                            let cy = obstacles[oi].y + OBS_H / 2;
                            let mut spawned = 0;
                            for p in particles.iter_mut() {
                                if p.life == 0 && spawned < 6 {
                                    p.x = cx + rng.range(OBS_W) - OBS_W / 2;
                                    p.y = cy + rng.range(OBS_H) - OBS_H / 2;
                                    p.dx = rng.range(7) - 3;
                                    p.dy = rng.range(7) - 3;
                                    if p.dx == 0 && p.dy == 0 {
                                        p.dy = -1;
                                    }
                                    p.life = PARTICLE_LIFE;
                                    spawned += 1;
                                }
                            }
                            missiles[mi].active = false;
                            obstacles[oi].active = false;
                            score += 2;
                            log::info!("Destroyed! Score: {}", score);
                            break;
                        }
                    }
                }

                // --- Player-obstacle collision ---
                if invincible > 0 {
                    invincible -= 1;
                } else {
                    for obs in obstacles.iter_mut() {
                        if !obs.active {
                            continue;
                        }
                        if aabb_overlap(
                            player_x, PLAYER_Y, PLAYER_W, PLAYER_H, obs.x, obs.y, OBS_W,
                            OBS_H,
                        ) {
                            obs.active = false;
                            lives = lives.saturating_sub(1);
                            invincible = 20;
                            log::info!("Hit! Lives: {}", lives);
                            if lives == 0 {
                                game_state = GameState::GameOver;
                                log::info!("Game Over! Score: {}", score);
                                break;
                            }
                        }
                    }
                }

                // --- Render game area (clear + redraw) ---
                Rectangle::new(
                    Point::new(0, HUD_H),
                    Size::new(SCREEN_W as u32, (SCREEN_H - HUD_H) as u32),
                )
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(&mut display)
                .unwrap();

                // Draw obstacles
                for obs in &obstacles {
                    if !obs.active {
                        continue;
                    }
                    Rectangle::new(
                        Point::new(obs.x, obs.y),
                        Size::new(OBS_W as u32, OBS_H as u32),
                    )
                    .into_styled(PrimitiveStyle::with_fill(obs_color))
                    .draw(&mut display)
                    .unwrap();
                }

                // Draw missiles
                for m in &missiles {
                    if !m.active {
                        continue;
                    }
                    Rectangle::new(
                        Point::new(m.x, m.y),
                        Size::new(MISSILE_W as u32, MISSILE_H as u32),
                    )
                    .into_styled(PrimitiveStyle::with_fill(missile_color))
                    .draw(&mut display)
                    .unwrap();
                }

                // Draw particles (debris)
                for p in &particles {
                    if p.life == 0 {
                        continue;
                    }
                    // Fade: white → yellow → red based on remaining life
                    let color = if p.life > 5 {
                        Rgb565::WHITE
                    } else if p.life > 2 {
                        Rgb565::YELLOW
                    } else {
                        Rgb565::RED
                    };
                    Rectangle::new(Point::new(p.x, p.y), Size::new(2, 2))
                        .into_styled(PrimitiveStyle::with_fill(color))
                        .draw(&mut display)
                        .unwrap();
                }

                // Draw player (blinks when invincible)
                if invincible == 0 || frame % 4 < 2 {
                    Rectangle::new(
                        Point::new(player_x, PLAYER_Y),
                        Size::new(PLAYER_W as u32, PLAYER_H as u32),
                    )
                    .into_styled(PrimitiveStyle::with_fill(player_color))
                    .draw(&mut display)
                    .unwrap();
                }

                // --- HUD: score (update only when changed) ---
                if score != prev_score {
                    Rectangle::new(Point::new(0, 0), Size::new(120, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display)
                        .unwrap();
                    buf.clear();
                    core::write!(buf, "Score: {}", score).ok();
                    Text::with_baseline(&buf, Point::new(4, 2), hud_style, Baseline::Top)
                        .draw(&mut display)
                        .unwrap();
                    prev_score = score;
                }

                // --- HUD: lives (update only when changed) ---
                if lives != prev_lives {
                    Rectangle::new(Point::new(200, 0), Size::new(40, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display)
                        .unwrap();
                    for i in 0..MAX_LIVES {
                        let color = if i < lives { life_on } else { life_off };
                        let x = 204 + (i as i32) * 12;
                        Rectangle::new(Point::new(x, 3), Size::new(8, 8))
                            .into_styled(PrimitiveStyle::with_fill(color))
                            .draw(&mut display)
                            .unwrap();
                    }
                    prev_lives = lives;
                }
            }

            // ==================== GAME OVER ====================
            GameState::GameOver => {
                if prev_state != GameState::GameOver {
                    display.clear(Rgb565::BLACK).unwrap();
                    Text::with_baseline(
                        "GAME OVER",
                        Point::new(75, 25),
                        gameover_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    buf.clear();
                    core::write!(buf, "Score: {}", score).ok();
                    Text::with_baseline(&buf, Point::new(90, 60), info_style, Baseline::Top)
                        .draw(&mut display)
                        .unwrap();
                    Text::with_baseline(
                        "Press any button",
                        Point::new(72, 90),
                        info_style,
                        Baseline::Top,
                    )
                    .draw(&mut display)
                    .unwrap();
                    led.set_low();
                    prev_state = GameState::GameOver;
                    log::info!("Game Over screen");
                }

                if a_just || b_just || x_just || y_just {
                    game_state = GameState::Title;
                }
            }
        }

        frame = frame.wrapping_add(1);

        // ~20 FPS frame timing
        Timer::at(frame_start + Duration::from_millis(50)).await;
    }
}
