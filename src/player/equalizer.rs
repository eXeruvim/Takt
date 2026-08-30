use crate::ipc::EqualizerPreset;

pub const MIN_PREAMP_DB: f64 = -12.0;
pub const MAX_PREAMP_DB: f64 = 6.0;
pub const MIN_BAND_GAIN_DB: f64 = -6.0;
pub const MAX_BAND_GAIN_DB: f64 = 6.0;

const FREQUENCIES_HZ: [u32; 5] = [60, 230, 910, 3_600, 14_000];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EqualizerSettings {
    pub enabled: bool,
    pub preset: EqualizerPreset,
    pub preamp_db: f64,
    pub custom_preamp_db: f64,
    pub custom_gains_db: [f64; 5],
}

impl EqualizerSettings {
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    pub fn with_preset(self, preset: EqualizerPreset) -> Self {
        if preset == EqualizerPreset::Custom {
            return Self {
                enabled: true,
                preset,
                preamp_db: self.custom_preamp_db,
                ..self
            };
        }

        Self {
            enabled: true,
            preset,
            preamp_db: recommended_preamp_db(preset),
            ..self
        }
    }

    pub fn adjusted_preamp(self, delta_db: f64) -> Self {
        let preamp_db = normalize_preamp_db(self.preamp_db + delta_db);
        Self {
            enabled: true,
            preamp_db,
            custom_preamp_db: if self.preset == EqualizerPreset::Custom {
                preamp_db
            } else {
                self.custom_preamp_db
            },
            ..self
        }
    }

    pub fn adjusted_band(self, index: usize, delta_db: f64) -> Option<Self> {
        let gain = self.custom_gains_db.get(index).copied()?;
        let mut custom_gains_db = self.custom_gains_db;
        custom_gains_db[index] = normalize_band_gain_db(gain + delta_db);

        Some(Self {
            enabled: true,
            preset: EqualizerPreset::Custom,
            preamp_db: self.custom_preamp_db,
            custom_gains_db,
            ..self
        })
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

        let gains = self.band_gains_db();
        let mut filters = Vec::with_capacity(FREQUENCIES_HZ.len() + 2);
        filters.push(format!("volume=volume={:.0}dB", self.preamp_db));
        filters.extend(
            FREQUENCIES_HZ
                .iter()
                .zip(gains)
                .map(|(frequency, gain)| format!("equalizer=f={frequency}:t=q:w=1:g={gain:.0}")),
        );
        // Presets can boost neighboring bands at the same time. The limiter is
        // deliberately last so those overlaps cannot clip the output.
        filters.push("alimiter=limit=0.95:level=false:latency=true".to_owned());

        Some(format!("@oxide-eq:lavfi=[{}]", filters.join(",")))
    }

    pub fn band_gains_db(self) -> [f64; 5] {
        if self.preset == EqualizerPreset::Custom {
            self.custom_gains_db
        } else {
            preset_gains_db(self.preset)
        }
    }
}

pub const fn recommended_preamp_db(preset: EqualizerPreset) -> f64 {
    match preset {
        EqualizerPreset::Flat => 0.0,
        EqualizerPreset::Bass => -4.0,
        EqualizerPreset::Voice => -3.0,
        EqualizerPreset::Rock => -3.0,
        EqualizerPreset::Electronic => -4.0,
        EqualizerPreset::Custom => 0.0,
    }
}

pub const fn preset_gains_db(preset: EqualizerPreset) -> [f64; 5] {
    match preset {
        EqualizerPreset::Flat => [0.0, 0.0, 0.0, 0.0, 0.0],
        EqualizerPreset::Bass => [4.0, 3.0, 0.0, -1.0, -2.0],
        EqualizerPreset::Voice => [-3.0, -1.0, 2.0, 3.0, 1.0],
        EqualizerPreset::Rock => [3.0, 1.0, -1.0, 2.0, 3.0],
        EqualizerPreset::Electronic => [4.0, 2.0, -1.0, 1.0, 4.0],
        EqualizerPreset::Custom => [0.0, 0.0, 0.0, 0.0, 0.0],
    }
}

fn normalize_preamp_db(value: f64) -> f64 {
    value.clamp(MIN_PREAMP_DB, MAX_PREAMP_DB).round()
}

fn normalize_band_gain_db(value: f64) -> f64 {
    value.clamp(MIN_BAND_GAIN_DB, MAX_BAND_GAIN_DB).round()
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
        assert_eq!(settings.custom_preamp_db, 0.0);
        assert_eq!(settings.custom_gains_db, [0.0; 5]);
    }

    #[test]
    fn custom_profile_is_independent_from_builtin_presets() {
        let settings = EqualizerSettings::default()
            .with_preset(EqualizerPreset::Custom)
            .adjusted_band(0, 2.0)
            .expect("known band")
            .adjusted_preamp(-2.0)
            .with_preset(EqualizerPreset::Bass)
            .with_preset(EqualizerPreset::Custom);

        assert_eq!(settings.custom_gains_db, [2.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(settings.preamp_db, -2.0);
        assert_eq!(settings.custom_preamp_db, -2.0);
    }

    #[test]
    fn band_adjustment_selects_custom_and_clamps_gain() {
        let settings = EqualizerSettings::default()
            .with_preset(EqualizerPreset::Custom)
            .adjusted_band(0, 100.0)
            .expect("known band");

        assert_eq!(settings.preset, EqualizerPreset::Custom);
        assert_eq!(settings.custom_gains_db, [6.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(settings.preamp_db, 0.0);
        assert_eq!(settings.adjusted_band(5, 1.0), None);
    }
}
