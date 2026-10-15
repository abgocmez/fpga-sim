//! A small text card that stands for a module without a display: the sensor
//! models show their physical value this way in the GUI and in `pmod_frame`.

use crate::{DeviceImage, font5x7};

const W: usize = 128;
const H: usize = 32;

/// A 128x32 picture of up to four lines (about 21 characters each) in the
/// shared 5x7 font. Characters without a glyph show as a box.
pub fn readout(lines: &[&str]) -> DeviceImage {
    let mut rgba = Vec::with_capacity(W * H * 4);
    let mut lit = vec![false; W * H];
    for (row, line) in lines.iter().take(H / 8).enumerate() {
        for (i, c) in line.bytes().take(W / 6).enumerate() {
            for (cx, col) in font5x7::glyph(c).iter().enumerate() {
                for bit in 0..7 {
                    if col >> bit & 1 != 0 {
                        lit[(row * 8 + bit) * W + i * 6 + cx] = true;
                    }
                }
            }
        }
    }
    for on in lit {
        if on {
            rgba.extend_from_slice(&[150, 235, 165, 255]);
        } else {
            rgba.extend_from_slice(&[6, 14, 9, 255]);
        }
    }
    DeviceImage {
        width: W as u32,
        height: H as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lands_in_the_right_cells() {
        let img = readout(&["A", "", "1"]);
        assert_eq!((img.width, img.height), (128, 32));
        assert_eq!(img.rgba.len(), 128 * 32 * 4);
        let px = |x: usize, y: usize| img.rgba[(y * 128 + x) * 4] > 100;
        let a = font5x7::glyph(b'A');
        for (cx, col) in a.iter().enumerate() {
            for bit in 0..7 {
                assert_eq!(px(cx, bit), col >> bit & 1 != 0);
            }
        }
        let one = font5x7::glyph(b'1');
        assert_eq!(
            px(1, 16 + 1),
            one[1] & 2 != 0,
            "third line starts at row 16"
        );
        assert!(!px(100, 20), "the rest is dark");
    }
}
