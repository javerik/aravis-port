/// Generates a simple diagonal-gradient Mono8 test pattern, enough to give a streaming test
/// something non-trivial (and frame-dependent) to verify.
pub fn generate_mono8(width: u32, height: u32, frame_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = (x.wrapping_add(y).wrapping_add(frame_id as u32) % 255) as u8;
            data.push(value);
        }
    }
    data
}

/// The same diagonal gradient as [`generate_mono8`], but as little-endian `u16` pixels for the
/// unpacked >8-bit formats (Mono10/Mono12/Mono16). The ramp is stretched by 8 per step so a
/// 16-bit frame actually exercises values above 255, then masked to `bits` the way a sensor with
/// that depth would never set the high bits.
pub fn generate_mono16(width: u32, height: u32, frame_id: u64, bits: u32) -> Vec<u8> {
    let mask: u32 = if bits >= 16 {
        0xffff
    } else {
        (1u32 << bits) - 1
    };
    let mut data = Vec::with_capacity((width * height * 2) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = (x
                .wrapping_add(y)
                .wrapping_add(frame_id as u32)
                .wrapping_mul(8)
                & mask) as u16;
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    data
}

/// Profiles in one cycle of the [`generate_profiles`] scene.
pub const PROFILE_CYCLE: u64 = 1500;
/// Where the box sits in the cycle and across the profile (as a fraction of the width), and
/// the laser shadow it casts next to it, where the sensor sees no line (z = 0).
const BOX_PROFILES: std::ops::Range<u64> = 200..600;
const BOX_U: std::ops::Range<f64> = 0.55..0.75;
const SHADOW_U: std::ops::Range<f64> = 0.75..0.80;
/// The dome: centred at this fraction of the width and profile of the cycle, with these radii.
const DOME_U: f64 = 0.25;
const DOME_PROFILE: f64 = 1100.0;
const DOME_RADIUS_U: f64 = 0.15;
const DOME_RADIUS_PROFILES: f64 = 200.0;

/// A Linescan3D frame, as a laser profiler streams it in `Coord3D_C16`: every row is one
/// profile and every value a height (little-endian `u16`). Profile `p = frame_id * height + row`
/// counts across frames, so consecutive frames continue one scene: a rippled floor that a box
/// (with a laser shadow, z = 0, beside it) and a dome pass over, every [`PROFILE_CYCLE`]
/// profiles. Valid heights stay within 9000..=33000.
pub fn generate_profiles(width: u32, height: u32, frame_id: u64) -> Vec<u8> {
    use std::f64::consts::TAU;
    let w = width.max(1) as f64;
    // sin(a + b) from per-column and per-row tables: two multiplies per pixel instead of a sin.
    let (col_sin, col_cos): (Vec<f64>, Vec<f64>) = (0..width)
        .map(|x| (TAU * 1.5 * x as f64 / w).sin_cos())
        .unzip();
    let mut data = Vec::with_capacity((width * height * 2) as usize);
    for row in 0..height {
        let p = frame_id
            .wrapping_mul(height as u64)
            .wrapping_add(row as u64);
        let phase = p % PROFILE_CYCLE;
        let (row_sin, row_cos) = (TAU * (p % 600) as f64 / 600.0).sin_cos();
        let in_box = BOX_PROFILES.contains(&phase);
        let dome_v = (phase as f64 - DOME_PROFILE) / DOME_RADIUS_PROFILES;
        for x in 0..width as usize {
            let u = x as f64 / w;
            if in_box && SHADOW_U.contains(&u) {
                data.extend_from_slice(&0u16.to_le_bytes());
                continue;
            }
            let mut z = 12_000.0 + 3_000.0 * (col_sin[x] * row_cos + col_cos[x] * row_sin);
            if in_box && BOX_U.contains(&u) {
                z += 15_000.0;
            }
            let dome_u = (u - DOME_U) / DOME_RADIUS_U;
            let r2 = dome_u * dome_u + dome_v * dome_v;
            if r2 < 1.0 {
                z += 18_000.0 * (1.0 - r2).sqrt();
            }
            data.extend_from_slice(&(z.round() as u16).to_le_bytes());
        }
    }
    data
}

/// The test pattern for whatever `PixelFormat` register value is currently set.
pub fn generate(width: u32, height: u32, frame_id: u64, pixel_format: u32) -> Vec<u8> {
    match pixel_format {
        crate::registers::pixel_format::MONO10 => generate_mono16(width, height, frame_id, 10),
        crate::registers::pixel_format::MONO16 => generate_mono16(width, height, frame_id, 16),
        crate::registers::pixel_format::COORD3D_C16 => generate_profiles(width, height, frame_id),
        _ => generate_mono8(width, height, frame_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::pixel_format;

    #[test]
    fn produces_the_expected_byte_count() {
        let data = generate_mono8(8, 4, 0);
        assert_eq!(data.len(), 32);
    }

    #[test]
    fn differs_across_frame_ids() {
        assert_ne!(generate_mono8(8, 4, 0), generate_mono8(8, 4, 1));
    }

    #[test]
    fn mono16_is_two_little_endian_bytes_per_pixel() {
        let data = generate_mono16(8, 4, 0, 16);
        assert_eq!(data.len(), 8 * 4 * 2);
        // Pixel (x=7, y=3) of frame 0 is (7 + 3) * 8 = 80.
        let last = u16::from_le_bytes([data[62], data[63]]);
        assert_eq!(last, 80);
    }

    #[test]
    fn mono16_exceeds_the_8_bit_range() {
        let data = generate_mono16(64, 64, 0, 16);
        let max = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .max()
            .unwrap();
        assert!(max > 255, "max {max}");
    }

    #[test]
    fn mono10_never_sets_bits_above_ten() {
        let data = generate_mono16(256, 64, 3, 10);
        assert!(data
            .chunks_exact(2)
            .all(|c| u16::from_le_bytes([c[0], c[1]]) < 1024));
    }

    #[test]
    fn the_register_value_picks_the_layout() {
        assert_eq!(generate(8, 4, 0, pixel_format::MONO8).len(), 32);
        assert_eq!(generate(8, 4, 0, pixel_format::MONO10).len(), 64);
        assert_eq!(generate(8, 4, 0, pixel_format::MONO16).len(), 64);
        assert_eq!(generate(8, 4, 0, pixel_format::COORD3D_C16).len(), 64);
    }

    fn u16s(data: &[u8]) -> Vec<u16> {
        data.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn profiles_continue_across_frames() {
        // Frame 2 of 4-row frames holds profiles 8..=11, the same as 1-row frames 8..=11.
        let stacked = generate_profiles(100, 4, 2);
        let single: Vec<u8> = (8..=11)
            .flat_map(|f| generate_profiles(100, 1, f))
            .collect();
        assert_eq!(stacked, single);
    }

    #[test]
    fn only_the_laser_shadow_is_invalid() {
        let width = 200;
        let cycle = u16s(&generate_profiles(width, PROFILE_CYCLE as u32, 0));
        for (i, &z) in cycle.iter().enumerate() {
            let (x, p) = ((i % width as usize) as f64, (i / width as usize) as u64);
            let shadow = BOX_PROFILES.contains(&p) && SHADOW_U.contains(&(x / width as f64));
            assert_eq!(z == 0, shadow, "profile {p}, column {x}: {z}");
            if !shadow {
                assert!(
                    (9_000..=33_000).contains(&z),
                    "profile {p}, column {x}: {z}"
                );
            }
        }
    }

    #[test]
    fn the_box_and_dome_rise_out_of_the_floor() {
        let width = 100;
        let at = |p: u64, x: usize| u16s(&generate_profiles(width, 1, p))[x];
        assert!(at(400, 65) > at(100, 65) + 10_000, "box");
        assert!(at(1100, 25) > at(1400, 25) + 10_000, "dome");
    }
}
