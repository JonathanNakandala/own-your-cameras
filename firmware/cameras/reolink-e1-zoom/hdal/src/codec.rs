//! The NA51089 embedded audio codec (EAC) record path, below HDAL. Stock opens capture with the codec's
//! automatic level control (ALC) and noise gate on; together they act as a compressor and gate: quiet
//! sounds are pulled up to the ALC target, the noise between them is muted, and no capture volume
//! changes the level. `cam` turns both off and sets the analog PGA gain from `mic_volume` instead
//! (`docs/hardware/audio.md`). Field layout from the kernel's `eac_get_ad_config`/`eac_set_load`.
//!
//! Pure register arithmetic; the adapter that maps the registers lives in the camera wiring.

/// Physical base of the EAC register block (`/proc/iomem` `f0640000.eac`).
pub const BASE: usize = 0xF064_0000;
/// Writing 1 latches the configuration; the bit clears when the codec has taken it.
pub const LOAD: usize = 0x04;
/// Record gains: PGA left bits 0..=4, right bits 8..=12 (`eac_dump_info` names the top byte digital gain 1).
pub const AD_GAIN: usize = 0x20;
/// Bit 0 ALC enable, bit 1 noise gate enable; ALC/NG levels in the upper fields.
pub const AD_DYNAMICS: usize = 0x30;

const PGA_MAX: u32 = 0x1F;
const PGA_MASK: u32 = PGA_MAX | PGA_MAX << 8;
const ALC_AND_NOISE_GATE: u32 = 0b11;

/// The PGA code for `mic_volume` 0..=100: the whole 5-bit range, linear in codes (about 1.4 dB a step at
/// codes 10..=18). Stock runs code 14 under the ALC; the default 100 is code 31.
pub fn pga(volume: u8) -> u32 {
    (u32::from(volume.min(100)) * PGA_MAX + 50) / 100
}

/// `AD_GAIN` with both PGA channels at `pga(volume)`, everything else kept.
pub fn ad_gain(current: u32, volume: u8) -> u32 {
    let code = pga(volume);
    (current & !PGA_MASK) | code | code << 8
}

/// `AD_DYNAMICS` with ALC and the noise gate off, their parameters kept.
pub fn ad_dynamics(current: u32) -> u32 {
    current & !ALC_AND_NOISE_GATE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_spans_the_five_bit_pga() {
        assert_eq!(pga(0), 0);
        assert_eq!(pga(70), 22);
        assert_eq!(pga(100), 31);
        assert_eq!(pga(255), 31);
    }

    #[test]
    fn only_the_pga_fields_and_the_two_enable_bits_change() {
        // Values read from the camera after a stock capture start.
        assert_eq!(ad_gain(0x7373_0E0E, 70), 0x7373_1616);
        assert_eq!(ad_gain(0x7373_0E0E, 100), 0x7373_1F1F);
        assert_eq!(ad_dynamics(0x0F0F_05C7), 0x0F0F_05C4);
        assert_eq!(ad_dynamics(0x0F0F_05C4), 0x0F0F_05C4);
    }
}
