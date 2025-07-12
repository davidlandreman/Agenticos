#!/usr/bin/env python3
import struct

# Create a temporary font file with a few characters for testing
# This creates a raw bitmap font (not FNT format) - just 256 chars * 16 bytes each

font_data = bytearray(256 * 16)

# A (0x41)
font_data[0x41 * 16 + 2] = 0b00111100
font_data[0x41 * 16 + 3] = 0b01100110
font_data[0x41 * 16 + 4] = 0b11000011
font_data[0x41 * 16 + 5] = 0b11000011
font_data[0x41 * 16 + 6] = 0b11111111
font_data[0x41 * 16 + 7] = 0b11000011
font_data[0x41 * 16 + 8] = 0b11000011
font_data[0x41 * 16 + 9] = 0b11000011

# H (0x48)
font_data[0x48 * 16 + 2] = 0b11000011
font_data[0x48 * 16 + 3] = 0b11000011
font_data[0x48 * 16 + 4] = 0b11000011
font_data[0x48 * 16 + 5] = 0b11111111
font_data[0x48 * 16 + 6] = 0b11000011
font_data[0x48 * 16 + 7] = 0b11000011
font_data[0x48 * 16 + 8] = 0b11000011
font_data[0x48 * 16 + 9] = 0b11000011

# e (0x65)
font_data[0x65 * 16 + 4] = 0b00111100
font_data[0x65 * 16 + 5] = 0b01100110
font_data[0x65 * 16 + 6] = 0b11111111
font_data[0x65 * 16 + 7] = 0b11000000
font_data[0x65 * 16 + 8] = 0b11000000
font_data[0x65 * 16 + 9] = 0b01100110
font_data[0x65 * 16 + 10] = 0b00111100

# l (0x6C)
font_data[0x6C * 16 + 1] = 0b00111000
font_data[0x6C * 16 + 2] = 0b00011000
font_data[0x6C * 16 + 3] = 0b00011000
font_data[0x6C * 16 + 4] = 0b00011000
font_data[0x6C * 16 + 5] = 0b00011000
font_data[0x6C * 16 + 6] = 0b00011000
font_data[0x6C * 16 + 7] = 0b00011000
font_data[0x6C * 16 + 8] = 0b00011000
font_data[0x6C * 16 + 9] = 0b00011000
font_data[0x6C * 16 + 10] = 0b00111100

# o (0x6F)
font_data[0x6F * 16 + 4] = 0b00111100
font_data[0x6F * 16 + 5] = 0b01100110
font_data[0x6F * 16 + 6] = 0b11000011
font_data[0x6F * 16 + 7] = 0b11000011
font_data[0x6F * 16 + 8] = 0b11000011
font_data[0x6F * 16 + 9] = 0b01100110
font_data[0x6F * 16 + 10] = 0b00111100

with open('assets/default_font.bin', 'wb') as f:
    f.write(font_data)

print("Created default_font.bin")