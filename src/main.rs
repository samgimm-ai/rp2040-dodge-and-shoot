//! Raspberry Pi Pico - "Dodge & Shoot!" Game
//!
//! Pico Display Pack buttons:
//!   A (GP12) = fire left, X (GP14) = fire right
//!   B (GP13) = move left, Y (GP15) = move right
//!   A+X simultaneous = bomb (destroy all obstacles)
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
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
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
const PLAYER_Y: i32 = 122;
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
const MAX_MISSILES: usize = 8;
// --- Bombs ---
const MAX_BOMBS: u8 = 3;

// --- Gifts ---
const GIFT_W: i32 = 10;
const GIFT_H: i32 = 10;
const GIFT_SPEED: i32 = 1;
const MAX_GIFTS: usize = 2;
const GIFT_MAX_LIFE: u8 = 80;
const GIFT_FADE_START: u8 = 20;

// --- Power-up durations (frames at 20 FPS) ---
const FREEZE_DURATION: u32 = 100;  // 5 seconds
const HOMING_DURATION: u32 = 200;  // 10 seconds
const LASER_DURATION: u32 = 100;   // 5 seconds
const SHIELD_DURATION: u32 = 160;  // 8 seconds

// --- Particles ---
const MAX_PARTICLES: usize = 36;
const PARTICLE_LIFE: u8 = 8;

// --- Lives ---
const MAX_LIVES: u8 = 3;

// --- HUD ---
const HUD_H: i32 = 24;

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
    const fn new() -> Self { Self { x: 0, y: 0, active: false } }
}

#[derive(Clone, Copy)]
struct Missile {
    x: i32,
    y: i32,
    active: bool,
    homing: bool,
}
impl Missile {
    const fn new() -> Self { Self { x: 0, y: 0, active: false, homing: false } }
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
    const fn new() -> Self { Self { x: 0, y: 0, dx: 0, dy: 0, life: 0 } }
}

#[derive(Clone, Copy)]
struct Gift {
    x: i32,
    y: i32,
    life: u8,
    active: bool,
}
impl Gift {
    const fn new() -> Self { Self { x: 0, y: 0, life: 0, active: false } }
}

// --- xorshift32 PRNG ---
struct Rng { state: u32 }
impl Rng {
    fn new(seed: u32) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        self.state = x; x
    }
    fn range(&mut self, max: i32) -> i32 { (self.next_u32() % max as u32) as i32 }
}

fn aabb_overlap(ax: i32, ay: i32, aw: i32, ah: i32, bx: i32, by: i32, bw: i32, bh: i32) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

fn spawn_particles(particles: &mut [Particle], rng: &mut Rng, cx: i32, cy: i32, count: u8) {
    let mut spawned = 0u8;
    for p in particles.iter_mut() {
        if p.life == 0 && spawned < count {
            p.x = cx + rng.range(10) - 5;
            p.y = cy + rng.range(10) - 5;
            p.dx = rng.range(7) - 3;
            p.dy = rng.range(7) - 3;
            if p.dx == 0 && p.dy == 0 { p.dy = -1; }
            p.life = PARTICLE_LIFE;
            spawned += 1;
        }
    }
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

    let usb_driver = Driver::new(p.USB, Irqs);
    unwrap!(spawner.spawn(logger_task(usb_driver)));
    Timer::after(Duration::from_secs(2)).await;
    log::info!("=== Dodge & Shoot Game ===");

    let mut led = Output::new(p.PIN_25, Level::Low);

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

    // Buttons: [A][X] top, [B][Y] bottom
    let btn_a = Input::new(p.PIN_12, Pull::Up);
    let btn_b = Input::new(p.PIN_13, Pull::Up);
    let btn_x = Input::new(p.PIN_14, Pull::Up);
    let btn_y = Input::new(p.PIN_15, Pull::Up);

    // --- Game variables ---
    let mut game_state = GameState::Title;
    let mut prev_state = GameState::Playing;
    let mut player_x: i32 = (SCREEN_W - PLAYER_W) / 2;
    let mut obstacles = [Obstacle::new(); MAX_OBS];
    let mut missiles = [Missile::new(); MAX_MISSILES];
    let mut particles = [Particle::new(); MAX_PARTICLES];
    let mut gifts = [Gift::new(); MAX_GIFTS];
    let mut score: u32 = 0;
    let mut lives: u8 = MAX_LIVES;
    let mut bombs: u8 = MAX_BOMBS;
    let mut spawn_timer: u32 = 0;
    let mut gift_spawn_timer: u32 = 0;
    let mut freeze_timer: u32 = 0;
    let mut homing_timer: u32 = 0;
    let mut laser_timer: u32 = 0;
    let mut shield_timer: u32 = 0;
    let mut rng = Rng::new(12345);
    let mut rng_seeded = false;
    let mut invincible: u32 = 0;
    let mut frame: u32 = 0;
    let mut demo_mode = false;
    let mut prev_score: u32 = u32::MAX;
    let mut prev_lives: u8 = u8::MAX;
    let mut prev_bombs: u8 = u8::MAX;
    let mut prev_power: u8 = u8::MAX;
    let mut prev_a = false;
    let mut prev_b = false;
    let mut prev_x = false;
    let mut prev_y = false;
    let mut buf = heapless::String::<32>::new();
    let mut high_score: u32 = 0;
    let mut speed_base_score: u32 = 0;

    // Text styles
    let big_yellow = MonoTextStyle::new(&FONT_10X20, Rgb565::YELLOW);
    let big_white = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let big_red = MonoTextStyle::new(&FONT_10X20, Rgb565::RED);

    // Colors
    let player_color = Rgb565::CYAN;
    let obs_color = Rgb565::RED;
    let missile_color = Rgb565::YELLOW;
    let homing_color = Rgb565::new(31, 40, 0);
    let laser_color = Rgb565::new(0, 63, 31);
    let life_on = Rgb565::RED;
    let life_off = Rgb565::new(4, 8, 4);
    let bomb_on = Rgb565::new(0, 31, 0);
    let bomb_off = Rgb565::new(2, 8, 2);

    log::info!("Entering game loop");

    loop {
        let frame_start = Instant::now();

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

        if !rng_seeded && (a_down || b_down || x_down || y_down) {
            rng = Rng::new(Instant::now().as_ticks() as u32);
            rng_seeded = true;
        }

        match game_state {
            // ==================== TITLE ====================
            GameState::Title => {
                if prev_state != GameState::Title {
                    display.clear(Rgb565::BLACK).unwrap();
                    Text::with_baseline("DODGE!", Point::new(80, 15), big_yellow, Baseline::Top)
                        .draw(&mut display).unwrap();
                    Text::with_baseline("B:Left Y:Right", Point::new(50, 45), big_white, Baseline::Top)
                        .draw(&mut display).unwrap();
                    Text::with_baseline("A:Fire X:Fire", Point::new(50, 70), big_white, Baseline::Top)
                        .draw(&mut display).unwrap();
                    Text::with_baseline("Press any button", Point::new(20, 105), big_white, Baseline::Top)
                        .draw(&mut display).unwrap();
                    led.set_low();
                    prev_state = GameState::Title;
                    log::info!("Title screen");
                }

                let start_demo = a_down && x_down;
                let start_game = !start_demo && (a_just || b_just || x_just || y_just);
                if start_demo || start_game {
                    demo_mode = start_demo;
                    player_x = (SCREEN_W - PLAYER_W) / 2;
                    for o in obstacles.iter_mut() { o.active = false; }
                    for m in missiles.iter_mut() { m.active = false; }
                    for p in particles.iter_mut() { p.life = 0; }
                    for g in gifts.iter_mut() { g.active = false; }
                    score = 0;
                    lives = MAX_LIVES;
                    bombs = MAX_BOMBS;
                    freeze_timer = 0;
                    homing_timer = 0;
                    laser_timer = 0;
                    shield_timer = 0;
                    spawn_timer = 0;
                    gift_spawn_timer = 0;
                    invincible = 0;
                    speed_base_score = 0;
                    prev_score = u32::MAX;
                    prev_lives = u8::MAX;
                    prev_bombs = u8::MAX;
                    prev_power = u8::MAX;
                    game_state = GameState::Playing;
                    log::info!("{} start!", if demo_mode { "Demo" } else { "Game" });
                }
            }

            // ==================== PLAYING ====================
            GameState::Playing => {
                if prev_state != GameState::Playing {
                    display.clear(Rgb565::BLACK).unwrap();
                    led.set_high();
                    if demo_mode {
                        let ds = MonoTextStyle::new(&FONT_6X10, Rgb565::new(8, 16, 8));
                        Text::with_baseline("DEMO", Point::new(105, 7), ds, Baseline::Top)
                            .draw(&mut display).unwrap();
                    }
                    prev_state = GameState::Playing;
                }

                // Demo exit
                if demo_mode && (a_just || b_just || x_just || y_just) {
                    game_state = GameState::Title;
                    frame = frame.wrapping_add(1);
                    Timer::at(frame_start + Duration::from_millis(50)).await;
                    continue;
                }

                // --- Input ---
                let (mv_l, mv_r, fire_l, fire_r, use_bomb) = if demo_mode {
                    let pcx = player_x + PLAYER_W / 2;
                    let mut al = false;
                    let mut ar = false;
                    let mut fl = false;
                    let mut fr = frame % 8 == 0;
                    let mut ab = false;
                    let mut oc = 0u8;
                    let mut ny = -1i32;
                    let mut nx = 0i32;
                    for obs in obstacles.iter() {
                        if obs.active {
                            oc += 1;
                            if obs.y > ny { nx = obs.x + OBS_W / 2; ny = obs.y; }
                        }
                    }
                    if oc >= 4 && bombs > 0 { ab = true; }
                    if ny >= 0 {
                        let dx = nx - pcx;
                        if ny > PLAYER_Y - 30 && dx.abs() < PLAYER_W + 4 {
                            if dx >= 0 { al = true; } else { ar = true; }
                        } else {
                            if dx > 4 { ar = true; }
                            else if dx < -4 { al = true; }
                            else { fl = frame % 2 == 0; fr = frame % 2 != 0; }
                        }
                    }
                    (al, ar, fl, fr, ab)
                } else {
                    let both = a_down && x_down;
                    let bj = both && (a_just || x_just);
                    (b_down, y_down, !both && a_just, !both && x_just, bj)
                };

                if mv_l { player_x = (player_x - PLAYER_SPEED).max(0); }
                if mv_r { player_x = (player_x + PLAYER_SPEED).min(SCREEN_W - PLAYER_W); }

                // --- Bomb ---
                if use_bomb && bombs > 0 {
                    bombs -= 1;
                    for obs in obstacles.iter_mut() {
                        if obs.active {
                            spawn_particles(&mut particles, &mut rng, obs.x + OBS_W / 2, obs.y + OBS_H / 2, 4);
                            obs.active = false;
                            score += 2;
                        }
                    }
                    speed_base_score = score;
                    log::info!("BOMB! left: {}, speed reset", bombs);
                }

                // --- Laser beam (auto-target nearest obstacle) ---
                let laser_on = laser_timer > 0 && (a_down || x_down || demo_mode);
                let mut laser_tx = 0i32;
                let mut laser_ty = 0i32;
                let mut laser_hit = false;
                if laser_on {
                    let pcx = player_x + PLAYER_W / 2;
                    let mut best = i32::MAX;
                    let mut ti: Option<usize> = None;
                    for (i, obs) in obstacles.iter().enumerate() {
                        if !obs.active { continue; }
                        let d = (obs.x + OBS_W / 2 - pcx).abs() + (obs.y + OBS_H / 2 - PLAYER_Y).abs();
                        if d < best { best = d; ti = Some(i); }
                    }
                    if let Some(i) = ti {
                        laser_tx = obstacles[i].x + OBS_W / 2;
                        laser_ty = obstacles[i].y + OBS_H / 2;
                        laser_hit = true;
                        spawn_particles(&mut particles, &mut rng, laser_tx, laser_ty, 3);
                        obstacles[i].active = false;
                        score += 2;
                    }
                }

                // --- Fire missiles (A=left, X=right) ---
                if !laser_on {
                    if fire_l {
                        for m in missiles.iter_mut() {
                            if !m.active {
                                m.x = player_x + 2;
                                m.y = PLAYER_Y - MISSILE_H;
                                m.active = true;
                                m.homing = homing_timer > 0;
                                break;
                            }
                        }
                    }
                    if fire_r {
                        for m in missiles.iter_mut() {
                            if !m.active {
                                m.x = player_x + PLAYER_W - 2 - MISSILE_W;
                                m.y = PLAYER_Y - MISSILE_H;
                                m.active = true;
                                m.homing = homing_timer > 0;
                                break;
                            }
                        }
                    }
                }

                // --- Obstacle speed (0 when frozen, reset by bomb) ---
                let progress = score.saturating_sub(speed_base_score);
                let speed = if freeze_timer > 0 { 0 } else {
                    (INITIAL_SPEED + (progress / 10) as i32).min(6)
                };

                // --- Spawn obstacles ---
                spawn_timer += 1;
                let interval = 30u32.saturating_sub((progress / 10) * 5).max(10);
                if spawn_timer >= interval {
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
                    if !obs.active { continue; }
                    obs.y += speed;
                    if obs.y > SCREEN_H { obs.active = false; score += 1; }
                }

                // --- Spawn gifts ---
                gift_spawn_timer += 1;
                if gift_spawn_timer >= 200 && rng.range(100) < 15 {
                    gift_spawn_timer = 0;
                    for g in gifts.iter_mut() {
                        if !g.active {
                            g.x = rng.range(SCREEN_W - GIFT_W);
                            g.y = HUD_H;
                            g.life = GIFT_MAX_LIFE;
                            g.active = true;
                            break;
                        }
                    }
                }

                // --- Move gifts ---
                for g in gifts.iter_mut() {
                    if !g.active { continue; }
                    g.y += GIFT_SPEED;
                    g.life = g.life.saturating_sub(1);
                    if g.life == 0 { g.active = false; }
                }

                // --- Move missiles (homing uses proportional navigation) ---
                for m in missiles.iter_mut() {
                    if !m.active { continue; }
                    m.y -= MISSILE_SPEED;
                    if m.homing {
                        let mcx = m.x + MISSILE_W / 2;
                        let mut best = i32::MAX;
                        let mut tx = mcx;
                        let mut ty = m.y;
                        for obs in obstacles.iter() {
                            if !obs.active { continue; }
                            let ocx = obs.x + OBS_W / 2;
                            let ocy = obs.y + OBS_H / 2;
                            let d = (ocy - m.y).abs() + (ocx - mcx).abs();
                            if d < best { best = d; tx = ocx; ty = ocy; }
                        }
                        // Proportional steering: calculate frames to intercept
                        let dy = m.y - ty;
                        let frames = (dy / (MISSILE_SPEED + speed)).max(1);
                        let dx = tx - mcx;
                        let mut turn = dx / frames;
                        if turn == 0 && dx != 0 { turn = if dx > 0 { 1 } else { -1 }; }
                        m.x += turn.clamp(-6, 6);
                    }
                    if m.y < HUD_H { m.active = false; }
                }

                // --- Update particles ---
                for p in particles.iter_mut() {
                    if p.life == 0 { continue; }
                    p.x += p.dx;
                    p.y += p.dy;
                    p.life -= 1;
                }

                // --- Missile-obstacle collision ---
                for mi in 0..MAX_MISSILES {
                    if !missiles[mi].active { continue; }
                    for oi in 0..MAX_OBS {
                        if !obstacles[oi].active { continue; }
                        if aabb_overlap(
                            missiles[mi].x, missiles[mi].y, MISSILE_W, MISSILE_H,
                            obstacles[oi].x, obstacles[oi].y, OBS_W, OBS_H,
                        ) {
                            spawn_particles(&mut particles, &mut rng,
                                obstacles[oi].x + OBS_W / 2, obstacles[oi].y + OBS_H / 2, 6);
                            missiles[mi].active = false;
                            obstacles[oi].active = false;
                            score += 2;
                            break;
                        }
                    }
                }

                // --- Missile-gift collision ---
                for mi in 0..MAX_MISSILES {
                    if !missiles[mi].active { continue; }
                    for gi in 0..MAX_GIFTS {
                        if !gifts[gi].active { continue; }
                        if aabb_overlap(
                            missiles[mi].x, missiles[mi].y, MISSILE_W, MISSILE_H,
                            gifts[gi].x, gifts[gi].y, GIFT_W, GIFT_H,
                        ) {
                            missiles[mi].active = false;
                            gifts[gi].active = false;
                            spawn_particles(&mut particles, &mut rng,
                                gifts[gi].x + GIFT_W / 2, gifts[gi].y + GIFT_H / 2, 4);
                            // Random power-up (6 types)
                            match rng.range(6) {
                                0 => { bombs = (bombs + 1).min(MAX_BOMBS); log::info!("Gift: Bomb+1"); }
                                1 => { lives = (lives + 1).min(MAX_LIVES); log::info!("Gift: Life+1"); }
                                2 => {
                                    freeze_timer = FREEZE_DURATION;
                                    // Remove obstacles near the bottom
                                    for oi in 0..MAX_OBS {
                                        if obstacles[oi].active && obstacles[oi].y + OBS_H >= PLAYER_Y - 5 {
                                            spawn_particles(&mut particles, &mut rng,
                                                obstacles[oi].x + OBS_W / 2, obstacles[oi].y + OBS_H / 2, 3);
                                            obstacles[oi].active = false;
                                        }
                                    }
                                    log::info!("Gift: Freeze!");
                                }
                                3 => { homing_timer = HOMING_DURATION; log::info!("Gift: Homing!"); }
                                4 => { laser_timer = LASER_DURATION; log::info!("Gift: Laser!"); }
                                _ => { shield_timer = SHIELD_DURATION; log::info!("Gift: Shield!"); }
                            }
                            break;
                        }
                    }
                }

                // --- Player-obstacle collision ---
                let shielded = shield_timer > 0 || invincible > 0;
                if invincible > 0 { invincible -= 1; }
                if !shielded {
                    for obs in obstacles.iter_mut() {
                        if !obs.active { continue; }
                        if aabb_overlap(
                            player_x, PLAYER_Y, PLAYER_W, PLAYER_H,
                            obs.x, obs.y, OBS_W, OBS_H,
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

                // --- Tick power-up timers ---
                freeze_timer = freeze_timer.saturating_sub(1);
                homing_timer = homing_timer.saturating_sub(1);
                laser_timer = laser_timer.saturating_sub(1);
                shield_timer = shield_timer.saturating_sub(1);

                // ==================== RENDER ====================
                Rectangle::new(
                    Point::new(0, HUD_H),
                    Size::new(SCREEN_W as u32, (SCREEN_H - HUD_H) as u32),
                )
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(&mut display).unwrap();

                // Laser beam (line to target)
                if laser_on && laser_hit {
                    let pcx = player_x + PLAYER_W / 2;
                    Line::new(Point::new(pcx, PLAYER_Y), Point::new(laser_tx, laser_ty))
                        .into_styled(PrimitiveStyle::with_stroke(laser_color, 1))
                        .draw(&mut display).unwrap();
                }

                // Obstacles (blue when frozen)
                for obs in &obstacles {
                    if !obs.active { continue; }
                    let c = if freeze_timer > 0 { Rgb565::BLUE } else { obs_color };
                    Rectangle::new(Point::new(obs.x, obs.y), Size::new(OBS_W as u32, OBS_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(c))
                        .draw(&mut display).unwrap();
                }

                // Gifts (blink when fading)
                for g in &gifts {
                    if !g.active { continue; }
                    if g.life <= GIFT_FADE_START && frame % 4 < 2 { continue; }
                    let c = if g.life > GIFT_FADE_START { Rgb565::GREEN } else { Rgb565::new(0, 20, 0) };
                    Rectangle::new(Point::new(g.x, g.y), Size::new(GIFT_W as u32, GIFT_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(c))
                        .draw(&mut display).unwrap();
                }

                // Missiles (orange when homing)
                for m in &missiles {
                    if !m.active { continue; }
                    let c = if m.homing { homing_color } else { missile_color };
                    Rectangle::new(Point::new(m.x, m.y), Size::new(MISSILE_W as u32, MISSILE_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(c))
                        .draw(&mut display).unwrap();
                }

                // Particles
                for p in &particles {
                    if p.life == 0 { continue; }
                    let c = if p.life > 5 { Rgb565::WHITE }
                        else if p.life > 2 { Rgb565::YELLOW }
                        else { Rgb565::RED };
                    Rectangle::new(Point::new(p.x, p.y), Size::new(2, 2))
                        .into_styled(PrimitiveStyle::with_fill(c))
                        .draw(&mut display).unwrap();
                }

                // Player (blinks: shield=white fast, invincible=cyan slow)
                let show = if shield_timer > 0 { frame % 3 != 0 }
                    else if invincible > 0 { frame % 4 < 2 }
                    else { true };
                if show {
                    let c = if shield_timer > 0 { Rgb565::WHITE } else { player_color };
                    Rectangle::new(
                        Point::new(player_x, PLAYER_Y),
                        Size::new(PLAYER_W as u32, PLAYER_H as u32),
                    )
                    .into_styled(PrimitiveStyle::with_fill(c))
                    .draw(&mut display).unwrap();
                }

                // --- HUD: score (big) ---
                if score != prev_score {
                    Rectangle::new(Point::new(0, 0), Size::new(100, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display).unwrap();
                    buf.clear();
                    core::write!(buf, "{}", score).ok();
                    let score_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
                    Text::with_baseline(&buf, Point::new(4, 2), score_style, Baseline::Top)
                        .draw(&mut display).unwrap();
                    prev_score = score;
                }

                // --- HUD: bombs ---
                if bombs != prev_bombs {
                    Rectangle::new(Point::new(100, 0), Size::new(35, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display).unwrap();
                    for i in 0..MAX_BOMBS {
                        let c = if i < bombs { bomb_on } else { bomb_off };
                        Rectangle::new(Point::new(102 + (i as i32) * 10, 7), Size::new(7, 8))
                            .into_styled(PrimitiveStyle::with_fill(c))
                            .draw(&mut display).unwrap();
                    }
                    prev_bombs = bombs;
                }

                // --- HUD: active power-ups ---
                let pwr = (if freeze_timer > 0 { 1u8 } else { 0 })
                    | (if homing_timer > 0 { 2 } else { 0 })
                    | (if laser_timer > 0 { 4 } else { 0 })
                    | (if shield_timer > 0 { 8 } else { 0 });
                if pwr != prev_power {
                    Rectangle::new(Point::new(135, 0), Size::new(60, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display).unwrap();
                    let mut ix = 137i32;
                    if freeze_timer > 0 {
                        let s = MonoTextStyle::new(&FONT_6X10, Rgb565::BLUE);
                        Text::with_baseline("F", Point::new(ix, 7), s, Baseline::Top)
                            .draw(&mut display).unwrap();
                        ix += 10;
                    }
                    if homing_timer > 0 {
                        let s = MonoTextStyle::new(&FONT_6X10, homing_color);
                        Text::with_baseline("H", Point::new(ix, 7), s, Baseline::Top)
                            .draw(&mut display).unwrap();
                        ix += 10;
                    }
                    if laser_timer > 0 {
                        let s = MonoTextStyle::new(&FONT_6X10, laser_color);
                        Text::with_baseline("L", Point::new(ix, 7), s, Baseline::Top)
                            .draw(&mut display).unwrap();
                        ix += 10;
                    }
                    if shield_timer > 0 {
                        let s = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
                        Text::with_baseline("S", Point::new(ix, 7), s, Baseline::Top)
                            .draw(&mut display).unwrap();
                    }
                    prev_power = pwr;
                }

                // --- HUD: lives ---
                if lives != prev_lives {
                    Rectangle::new(Point::new(200, 0), Size::new(40, HUD_H as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display).unwrap();
                    for i in 0..MAX_LIVES {
                        let c = if i < lives { life_on } else { life_off };
                        Rectangle::new(Point::new(204 + (i as i32) * 12, 7), Size::new(8, 8))
                            .into_styled(PrimitiveStyle::with_fill(c))
                            .draw(&mut display).unwrap();
                    }
                    prev_lives = lives;
                }
            }

            // ==================== GAME OVER ====================
            GameState::GameOver => {
                if prev_state != GameState::GameOver {
                    if score > high_score { high_score = score; }
                    display.clear(Rgb565::BLACK).unwrap();
                    Text::with_baseline("GAME OVER", Point::new(50, 10), big_red, Baseline::Top)
                        .draw(&mut display).unwrap();
                    buf.clear();
                    core::write!(buf, "{}", score).ok();
                    Text::with_baseline(&buf, Point::new(100, 40), big_yellow, Baseline::Top)
                        .draw(&mut display).unwrap();
                    buf.clear();
                    core::write!(buf, "Best: {}", high_score).ok();
                    Text::with_baseline(&buf, Point::new(60, 70), big_white, Baseline::Top)
                        .draw(&mut display).unwrap();
                    Text::with_baseline("Press any button", Point::new(20, 105), big_white, Baseline::Top)
                        .draw(&mut display).unwrap();
                    led.set_low();
                    prev_state = GameState::GameOver;
                    log::info!("Game Over screen");
                }

                if demo_mode {
                    if frame % 40 == 0 { game_state = GameState::Title; }
                } else if a_just || b_just || x_just || y_just {
                    game_state = GameState::Title;
                }
            }
        }

        frame = frame.wrapping_add(1);
        Timer::at(frame_start + Duration::from_millis(50)).await;
    }
}
