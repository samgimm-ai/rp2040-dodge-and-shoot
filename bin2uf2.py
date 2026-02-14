#!/usr/bin/env python3
"""Convert a raw binary to UF2 format for RP2350 ARM-S."""

import struct
import sys

UF2_MAGIC_START0 = 0x0A324655  # "UF2\n"
UF2_MAGIC_START1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY_ID = 0x00002000

RP2350_ARM_S_FAMILY = 0xE48BFF59
FLASH_BASE = 0x10000000
PAYLOAD_SIZE = 256


def bin_to_uf2(input_path, output_path, base_addr=FLASH_BASE, family_id=RP2350_ARM_S_FAMILY):
    with open(input_path, "rb") as f:
        data = f.read()

    num_blocks = (len(data) + PAYLOAD_SIZE - 1) // PAYLOAD_SIZE
    blocks = []

    for i in range(num_blocks):
        chunk = data[i * PAYLOAD_SIZE:(i + 1) * PAYLOAD_SIZE]
        # Pad to PAYLOAD_SIZE
        chunk = chunk.ljust(PAYLOAD_SIZE, b'\x00')

        addr = base_addr + i * PAYLOAD_SIZE
        # 32 bytes header + 476 bytes data area + 4 bytes footer = 512 bytes
        block = struct.pack(
            "<IIIIIIII",
            UF2_MAGIC_START0,
            UF2_MAGIC_START1,
            UF2_FLAG_FAMILY_ID,
            addr,
            PAYLOAD_SIZE,
            i,
            num_blocks,
            family_id,
        )
        block += chunk
        block += b'\x00' * (476 - PAYLOAD_SIZE)  # pad data area to 476 bytes
        block += struct.pack("<I", UF2_MAGIC_END)

        assert len(block) == 512
        blocks.append(block)

    with open(output_path, "wb") as f:
        for block in blocks:
            f.write(block)

    print(f"Converted {len(data)} bytes -> {num_blocks} UF2 blocks ({num_blocks * 512} bytes)")
    print(f"Base address: 0x{base_addr:08x}, Family ID: 0x{family_id:08x}")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <input.bin> <output.uf2>")
        sys.exit(1)
    bin_to_uf2(sys.argv[1], sys.argv[2])
