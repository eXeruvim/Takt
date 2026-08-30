use crate::ipc::EqualizerPreset;

pub const MIN_PREAMP_DB: f64 = -12.0;
pub const MAX_PREAMP_DB: f64 = 6.0;

const FREQUENCIES_HZ: [u32; 5] = [60, 230, 910, 3_600, 14_000];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EqualizerSettings {
    pub enabled: bool,
    pub preset: EqualizerPreset,
    pub preamp_db: f64,
}

impl EqualizerSettings {
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    pub fn with_preset(self, preset: EqualizerPreset) -> Self {
        Self {
            enabled: true,
            preset,
            preamp_db: recommended_preamp_db(preset),
        }
    }

    pub fn adjusted_preamp(self, delta_db: f64) -> Self {
        Self {
            enabled: true,
            preamp_db: normalize_preamp_db(self.preamp_db + delta_db),
            ..self
        }
    }

    pub fn reset(self) -> Self {
        Self {
            enabled: self.enabled,
            ..Self::default()
        }
    }

    pub fn mpv_filter(self) -> Option<String> {
        if !self.enabled {
            return None;
        }

        let gains = preset_gains_db(self.preset);
        let mut filters = Vec::with_capacity(FREQUENCIES_HZ.len() + 2);
        filters.push(format!("volume=volume={:.0}dB", self.preamp_db));
        filters.extend(
            FREQUENCIES_HZ
                .iter()
                .zip(gains)
                .map(|(frequency, gain)| format!("equalizer=f={frequency}:t=q:w=1:g={gain}")),
        );
        // Presets can boost neighboring bands at the same time. The limiter is
        // deliberately last so those overlaps cannot clip the output.
        filters.push("alimiter=limit=0.95:level=false:latency=true".to_owned());

        Some(format!("@oxide-eq:lavfi=[{}]", filters.join(",")))
    }
}

pub const fn recommended_preamp_db(preset: EqualizerPreset) -> f64 {
    match preset {
        EqualizerPreset::Flat => 0.0,
        EqualizerPreset::Bass => -4.0,
        EqualizerPreset::Voice => -3.0,
        EqualizerPreset::Rock => -3.0,
        EqualizerPreset::Electronic => -4.0,
    }
}

pub const fn preset_gains_db(preset: EqualizerPreset) -> [i8; 5] {
    match preset {
        EqualizerPreset::Flat => [0, 0, 0, 0, 0],
        EqualizerPreset::Bass => [4, 3, 0, -1, -2],
        EqualizerPreset::Voice => [-3, -1, 2, 3, 1],
        EqualizerPreset::Rock => [3, 1, -1, 2, 3],
        EqualizerPreset::Electronic => [4, 2, -1, 1, 4],
    }
}

fn normalize_preamp_db(value: f64) -> f64 {
    value.clamp(MIN_PREAMP_DB, MAX_PREAMP_DB).round()
}

#[cfg(test)]
mod tests {
    use super::{EqualizerSettings, MAX_PREAMP_DB, MIN_PREAMP_DB};
    use crate::ipc::EqualizerPreset;

    #[test]
    fn bypass_does_not_create_an_audio_filter() {
        assert_eq!(EqualizerSettings::default().mpv_filter(), None);
    }

    #[test]
    fn preset_filter_has_headroom_bands_and_limiter() {
        let settings = EqualizerSettings::default().with_preset(EqualizerPreset::Bass);
        let filter = settings.mpv_filter().expect("enabled equalizer filter");

        assert!(filter.starts_with("@oxide-eq:lavfi=[volume=volume=-4dB"));
        assert_eq!(filter.matches("equalizer=f=").count(), 5);
        assert!(filter.contains("equalizer=f=60:t=q:w=1:g=4"));
        assert!(filter.ends_with("alimiter=limit=0.95:level=false:latency=true]"));
    }

    #[test]
    fn preamp_is_rounded_and_clamped_to_safe_ui_range() {
        let settings = EqualizerSettings::default()
            .adjusted_preamp(100.0)
            .adjusted_preamp(-0.4);
        assert_eq!(settings.preamp_db, MAX_PREAMP_DB);

        let settings = settings.adjusted_preamp(-100.0);
        assert_eq!(settings.preamp_db, MIN_PREAMP_DB);
    }

    #[test]
    fn reset_keeps_bypass_choice_but_clears_tone_changes() {
        let settings = EqualizerSettings::default()
            .with_preset(EqualizerPreset::Rock)
            .with_enabled(false)
            .reset();

        assert!(!settings.enabled);
        assert_eq!(settings.preset, EqualizerPreset::Flat);
        assert_eq!(settings.preamp_db, 0.0);
    }
}
