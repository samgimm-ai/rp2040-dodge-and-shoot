# Dodge & Shoot!

An embedded shooting game built with Raspberry Pi Pico + Pico Display Pack.

Written in Rust with the Embassy async runtime, running in a `no_std` environment.

[한국어 README는 아래에 있습니다.](#dodge--shoot-한국어)

## How to Play

Dodge falling obstacles or destroy them with missiles!

| Button | Action |
|--------|--------|
| **B** (GP13) | Move left |
| **Y** (GP15) | Move right |
| **A** (GP12) / **X** (GP14) | Fire missile |

- +1 point for dodging an obstacle, +2 for destroying it
- 3 lives, 20 frames of invincibility after being hit
- Obstacle speed and spawn rate increase every 10 points

## Hardware

- [Raspberry Pi Pico](https://www.raspberrypi.com/products/raspberry-pi-pico/) (RP2040)
- [Pico Display Pack](https://shop.pimoroni.com/products/pico-display-pack) (ST7789, 240x135, 4 buttons)

### Pin Map

| Function | Pin |
|----------|-----|
| Display SPI CLK | GP18 |
| Display SPI MOSI | GP19 |
| Display CS | GP17 |
| Display DC | GP16 |
| Backlight | GP20 |
| Onboard LED | GP25 |
| Button A | GP12 |
| Button B | GP13 |
| Button X | GP14 |
| Button Y | GP15 |

## Build & Flash

### Prerequisites

```bash
# Add Rust target
rustup target add thumbv6m-none-eabi

# Install UF2 conversion tool
cargo install elf2uf2-rs
```

### Build & Flash

1. Hold the **BOOTSEL** button on the Pico while plugging in USB
2. Run:

```bash
./deploy.sh
```

Or manually:

```bash
cargo build --release
elf2uf2-rs convert --family rp2040 target/thumbv6m-none-eabi/release/rasp-pico-hello target/rasp-pico-hello.uf2
cp target/rasp-pico-hello.uf2 /Volumes/RPI-RP2/
```

## Tech Stack

| Item | Detail |
|------|--------|
| Language | Rust (2024 Edition, `no_std`) |
| MCU | RP2040 (ARM Cortex-M0+) |
| Async Runtime | Embassy |
| Display Driver | mipidsi (ST7789) |
| Graphics | embedded-graphics |
| Logging | USB serial (defmt) |

## License

MIT

---

# Dodge & Shoot! (한국어)

Raspberry Pi Pico + Pico Display Pack으로 만든 임베디드 슈팅 게임입니다.

Rust와 Embassy async 런타임으로 작성되었으며, `no_std` 환경에서 동작합니다.

## 게임 방법

떨어지는 장애물을 피하거나 미사일로 파괴하세요!

| 버튼 | 동작 |
|------|------|
| **B** (GP13) | 왼쪽 이동 |
| **Y** (GP15) | 오른쪽 이동 |
| **A** (GP12) / **X** (GP14) | 미사일 발사 |

- 장애물 회피 시 +1점, 파괴 시 +2점
- 라이프 3개, 피격 시 20프레임 무적
- 10점마다 장애물 속도와 스폰 빈도 증가

## 하드웨어

- [Raspberry Pi Pico](https://www.raspberrypi.com/products/raspberry-pi-pico/) (RP2040)
- [Pico Display Pack](https://shop.pimoroni.com/products/pico-display-pack) (ST7789, 240x135, 버튼 4개)

### 핀 배치

| 기능 | 핀 |
|------|-----|
| 디스플레이 SPI CLK | GP18 |
| 디스플레이 SPI MOSI | GP19 |
| 디스플레이 CS | GP17 |
| 디스플레이 DC | GP16 |
| 백라이트 | GP20 |
| 온보드 LED | GP25 |
| 버튼 A | GP12 |
| 버튼 B | GP13 |
| 버튼 X | GP14 |
| 버튼 Y | GP15 |

## 빌드 및 플래싱

### 사전 준비

```bash
# Rust 타겟 추가
rustup target add thumbv6m-none-eabi

# UF2 변환 도구 설치
cargo install elf2uf2-rs
```

### 빌드 & 플래싱

1. Pico의 **BOOTSEL** 버튼을 누른 채 USB 연결
2. 실행:

```bash
./deploy.sh
```

또는 수동으로:

```bash
cargo build --release
elf2uf2-rs convert --family rp2040 target/thumbv6m-none-eabi/release/rasp-pico-hello target/rasp-pico-hello.uf2
cp target/rasp-pico-hello.uf2 /Volumes/RPI-RP2/
```

## 기술 스택

| 항목 | 내용 |
|------|------|
| 언어 | Rust (2024 Edition, `no_std`) |
| MCU | RP2040 (ARM Cortex-M0+) |
| Async 런타임 | Embassy |
| 디스플레이 드라이버 | mipidsi (ST7789) |
| 그래픽 | embedded-graphics |
| 로깅 | USB serial (defmt) |

## 라이선스

MIT
