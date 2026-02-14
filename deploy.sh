#!/bin/bash
# Build and deploy to Raspberry Pi Pico (RP2040)
set -e

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="thumbv6m-none-eabi"
BIN_NAME="rasp-pico-hello"
ELF="$PROJECT_DIR/target/$TARGET/release/$BIN_NAME"
UF2="$PROJECT_DIR/target/${BIN_NAME}.uf2"

echo "=== Building release ==="
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

echo "=== Converting ELF to UF2 ==="
elf2uf2-rs convert --family rp2040 "$ELF" "$UF2"

# Find mounted Pico (BOOTSEL mode)
PICO_MOUNT=""
for mount in /Volumes/RPI-RP2; do
    if [ -d "$mount" ]; then
        PICO_MOUNT="$mount"
        break
    fi
done

if [ -z "$PICO_MOUNT" ]; then
    echo ""
    echo "=== Pico not found in BOOTSEL mode ==="
    echo "To deploy:"
    echo "  1. Hold BOOTSEL button while plugging in USB"
    echo "  2. Copy the UF2 file:"
    echo "     cp $UF2 /Volumes/RPI-RP2/"
    echo ""
    echo "UF2 file ready at: $UF2"
    exit 0
fi

echo "=== Deploying to $PICO_MOUNT ==="
cp "$UF2" "$PICO_MOUNT/"
echo "Done! Pico should reboot and start the game."
