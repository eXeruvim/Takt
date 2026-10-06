//! Narrow libmpv setup and property adapter used by the player state machine.

use std::sync::Arc;

use anyhow::Context;
use libmpv::{Format, Mpv, events::EventContext};

pub(super) fn mpv_error<T>(result: Result<T, libmpv::Error>) -> anyhow::Result<T> {
    result.map_err(|error| anyhow::anyhow!("{error:?}"))
}

pub(super) fn create() -> anyhow::Result<Arc<Mpv>> {
    Mpv::with_initializer(|initializer| {
        configure_seek_osd(&initializer)?;
        // Oxide renders embedded artwork inside its own audio scene. Prevent mpv
        // from promoting the attached picture to the native video surface.
        initializer.set_property("audio-display", "no".to_owned())?;
        // Keep the audio device open for compatible consecutive tracks without
        // forcing unlike files into the first track's output format.
        initializer.set_property("gapless-audio", "weak".to_owned())?;
        // Software decoding is the reliable default. Hardware decoding can
        // be enabled at runtime for demanding files on compatible drivers.
        initializer.set_property("hwdec", "no".to_owned())?;
        initializer.set_property(
            "reset-on-next-file",
            "audio-delay,sub-delay,video-aspect-override".to_owned(),
        )?;
        #[cfg(target_os = "windows")]
        {
            initializer.set_property("vo", "libmpv".to_owned())?;
            // Delay the expensive OpenGL/libmpv/NVIDIA context until mpv
            // has confirmed a real video track. Audio, including a renamed
            // file with a video extension, stays on the lightweight path.
            initializer.set_property("vid", "no".to_owned())?;
            initializer.set_property("background", "color".to_owned())?;
            initializer
                .set_property("background-color", video_background_color(true).to_owned())?;
        }
        Ok(())
    })
    .map(Arc::new)
    .map_err(|error| anyhow::anyhow!("{error:?}"))
}

pub(super) fn observe_playback_properties(events: &mut EventContext<'_>) -> anyhow::Result<()> {
    for (id, name, format) in [
        (1, "time-pos", Format::Double),
        (2, "duration", Format::Double),
        (3, "pause", Format::Flag),
        (4, "volume", Format::Double),
        (5, "speed", Format::Double),
        (6, "mute", Format::Flag),
        (7, "loop-file", Format::String),
        (8, "audio-delay", Format::Double),
        (9, "sub-delay", Format::Double),
        (10, "chapter", Format::Int64),
    ] {
        mpv_error(events.observe_property(name, format, id))?;
    }
    Ok(())
}

pub(super) fn video_background_color(dark_theme: bool) -> &'static str {
    // Keep these values aligned with Theme.media-canvas-background. mpv owns
    // the letterbox around video; Slint owns the corresponding audio surround.
    if dark_theme {
        "0.035/0.035/0.05/0.72"
    } else {
        "0.953/0.933/0.914/0.44"
    }
}

pub(super) fn hwdec_is_enabled(value: &str) -> bool {
    !matches!(value, "" | "no" | "false" | "0")
}

pub(super) fn automatic_hwdec_mode() -> &'static str {
    // Oxide's Windows renderer presents libmpv frames through OpenGL. Copy-back
    // avoids driver-specific D3D11VA/OpenGL interop corruption while keeping the
    // decode stage on the GPU. Other platforms can use direct safe interop.
    if cfg!(target_os = "windows") {
        "auto-copy-safe"
    } else {
        "auto-safe"
    }
}

pub(super) fn set_loaded_media_paused(mpv: &Mpv, paused: bool) -> anyhow::Result<()> {
    mpv_error(mpv.set_property("pause", paused)).context(if paused {
        "Не удалось приостановить файл до выбора позиции продолжения"
    } else {
        "Не удалось начать воспроизведение загруженного файла"
    })
}

fn configure_seek_osd(initializer: &libmpv::MpvInitializer) -> Result<(), libmpv::Error> {
    // Oxide's timeline already shows seeking. Disable mpv's duplicate overlay,
    // not subtitle rendering or the application's own feedback.
    initializer.set_property("osd-on-seek", "no")?;
    initializer.set_property("osd-bar", false)
}

#[cfg(test)]
mod tests {
    use super::{
        automatic_hwdec_mode, configure_seek_osd, hwdec_is_enabled, set_loaded_media_paused,
        video_background_color,
    };

    #[test]
    fn native_seek_overlay_is_disabled_without_hiding_subtitles() {
        let mpv = libmpv::Mpv::with_initializer(|initializer| {
            initializer.set_property("config", "no")?;
            initializer.set_property("load-scripts", "no")?;
            initializer.set_property("terminal", "no")?;
            initializer.set_property("vo", "null")?;
            initializer.set_property("ao", "null")?;
            initializer.set_property("sub-visibility", true)?;
            configure_seek_osd(&initializer)
        })
        .expect("headless mpv must accept the seek OSD options");
        assert!(!mpv.get_property::<bool>("osd-bar").unwrap());
        assert_eq!(mpv.get_property::<String>("osd-on-seek").unwrap(), "no");
        assert!(mpv.get_property::<bool>("sub-visibility").unwrap());
    }

    #[test]
    fn loaded_media_autoplays_unless_resume_choice_is_pending() {
        let mpv = libmpv::Mpv::with_initializer(|initializer| {
            initializer.set_property("config", "no")?;
            initializer.set_property("load-scripts", "no")?;
            initializer.set_property("terminal", "no")?;
            initializer.set_property("vo", "null")?;
            initializer.set_property("ao", "null")
        })
        .expect("headless mpv must initialize");

        set_loaded_media_paused(&mpv, true).unwrap();
        assert!(mpv.get_property::<bool>("pause").unwrap());

        set_loaded_media_paused(&mpv, false).unwrap();
        assert!(!mpv.get_property::<bool>("pause").unwrap());
    }

    #[test]
    fn recognizes_enabled_hardware_decoding_modes() {
        assert!(hwdec_is_enabled("auto-safe"));
        assert!(hwdec_is_enabled("d3d11va"));
        assert!(!hwdec_is_enabled("no"));
        assert!(!hwdec_is_enabled("false"));
        assert!(!hwdec_is_enabled("0"));
    }

    #[test]
    fn chooses_copy_back_hardware_decoding_for_windows_opengl() {
        assert_eq!(
            automatic_hwdec_mode(),
            if cfg!(target_os = "windows") {
                "auto-copy-safe"
            } else {
                "auto-safe"
            }
        );
    }

    #[test]
    fn chooses_letterbox_color_for_the_active_theme() {
        assert_eq!(video_background_color(true), "0.035/0.035/0.05/0.72");
        assert_eq!(video_background_color(false), "0.953/0.933/0.914/0.44");
    }
}
