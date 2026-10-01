use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState};

use crate::settings::ShortcutPreference;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
pub(crate) enum ShortcutAction {
    OpenFile = 0,
    OpenFolder = 1,
    PlayPause = 2,
    Previous = 3,
    Next = 4,
    ToggleMute = 5,
    ToggleLoop = 6,
    TogglePlaylist = 7,
    ToggleInfo = 8,
    ToggleSettings = 9,
    ToggleFullscreen = 11,
    Stop = 12,
    SeekBackwardLong = 13,
    SeekForwardLong = 14,
    VolumeDown = 15,
    VolumeUp = 16,
    SeekBackwardShort = 22,
    SeekForwardShort = 23,
    ToggleCommandPalette = 24,
    FrameBackward = 25,
    FrameForward = 26,
    TogglePictureInPicture = 27,
    OpenConverter = 28,
}

impl ShortcutAction {
    const ALL: [Self; 23] = [
        Self::ToggleCommandPalette,
        Self::OpenFile,
        Self::OpenFolder,
        Self::TogglePlaylist,
        Self::ToggleSettings,
        Self::PlayPause,
        Self::Stop,
        Self::Previous,
        Self::Next,
        Self::SeekBackwardLong,
        Self::SeekForwardLong,
        Self::SeekBackwardShort,
        Self::SeekForwardShort,
        Self::FrameBackward,
        Self::FrameForward,
        Self::VolumeDown,
        Self::VolumeUp,
        Self::ToggleMute,
        Self::ToggleLoop,
        Self::ToggleFullscreen,
        Self::TogglePictureInPicture,
        Self::OpenConverter,
        Self::ToggleInfo,
    ];

    pub(crate) fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|action| *action as i32 == id)
    }

    pub(crate) fn from_palette_action(action: i32) -> Option<Self> {
        Self::from_id(action)
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::OpenFile => "Открыть файл",
            Self::OpenFolder => "Открыть папку",
            Self::PlayPause => "Пауза / воспроизведение",
            Self::Stop => "Остановить воспроизведение",
            Self::Previous => "Предыдущий файл",
            Self::Next => "Следующий файл",
            Self::SeekBackwardShort => "Назад на 5 секунд",
            Self::SeekForwardShort => "Вперёд на 5 секунд",
            Self::SeekBackwardLong => "Назад на 10 секунд",
            Self::SeekForwardLong => "Вперёд на 10 секунд",
            Self::FrameBackward => "Предыдущий кадр",
            Self::FrameForward => "Следующий кадр",
            Self::VolumeDown => "Уменьшить громкость",
            Self::VolumeUp => "Увеличить громкость",
            Self::ToggleMute => "Выключить / включить звук",
            Self::ToggleLoop => "Повтор файла",
            Self::ToggleFullscreen => "Полноэкранный режим",
            Self::TogglePictureInPicture => "Картинка в картинке",
            Self::ToggleInfo => "Свойства файла",
            Self::TogglePlaylist => "Плейлист",
            Self::ToggleSettings => "Настройки воспроизведения",
            Self::ToggleCommandPalette => "Палитра команд",
            Self::OpenConverter => "Конвертировать файл",
        }
    }

    pub(crate) fn default_bindings(self) -> Vec<ShortcutBinding> {
        let plain = |code| ShortcutBinding::new(code, ModifiersState::empty());
        let control = |code| ShortcutBinding::new(code, ModifiersState::CONTROL);

        match self {
            Self::OpenFile => vec![control(KeyCode::KeyO), plain(KeyCode::KeyO)],
            Self::OpenFolder => vec![ShortcutBinding::new(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            )],
            Self::PlayPause => vec![plain(KeyCode::Space), plain(KeyCode::KeyK)],
            Self::Stop => vec![plain(KeyCode::KeyS)],
            Self::Previous => vec![plain(KeyCode::KeyP)],
            Self::Next => vec![plain(KeyCode::KeyN)],
            Self::SeekBackwardShort => vec![plain(KeyCode::ArrowLeft)],
            Self::SeekForwardShort => vec![plain(KeyCode::ArrowRight)],
            Self::SeekBackwardLong => vec![plain(KeyCode::KeyJ)],
            Self::SeekForwardLong => vec![plain(KeyCode::KeyL)],
            Self::FrameBackward => vec![plain(KeyCode::Comma)],
            Self::FrameForward => vec![plain(KeyCode::Period)],
            Self::VolumeDown => vec![plain(KeyCode::ArrowDown)],
            Self::VolumeUp => vec![plain(KeyCode::ArrowUp)],
            Self::ToggleMute => vec![plain(KeyCode::KeyM)],
            Self::ToggleLoop => vec![plain(KeyCode::KeyR)],
            Self::ToggleFullscreen => vec![plain(KeyCode::KeyF)],
            Self::TogglePictureInPicture => {
                vec![ShortcutBinding::new(KeyCode::KeyP, ModifiersState::ALT)]
            }
            Self::ToggleInfo => vec![plain(KeyCode::KeyI)],
            Self::TogglePlaylist => vec![control(KeyCode::KeyL)],
            Self::ToggleSettings | Self::OpenConverter => Vec::new(),
            Self::ToggleCommandPalette => vec![control(KeyCode::KeyK)],
        }
    }

    pub(crate) fn allows_repeat(self) -> bool {
        matches!(
            self,
            Self::SeekBackwardShort
                | Self::SeekForwardShort
                | Self::SeekBackwardLong
                | Self::SeekForwardLong
                | Self::VolumeDown
                | Self::VolumeUp
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShortcutBinding {
    code: KeyCode,
    modifiers: ModifiersState,
}

impl ShortcutBinding {
    pub(crate) fn new(code: KeyCode, modifiers: ModifiersState) -> Self {
        Self { code, modifiers }
    }

    pub(crate) fn display(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.control_key() {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.shift_key() {
            parts.push("Shift".to_string());
        }
        if self.modifiers.alt_key() {
            parts.push("Alt".to_string());
        }
        if self.modifiers.super_key() {
            parts.push("Win".to_string());
        }
        parts.push(key_code_label(self.code));
        parts.join("+")
    }
}

#[derive(Debug)]
pub(crate) struct ShortcutConflict {
    pub(crate) action: ShortcutAction,
    pub(crate) binding: ShortcutBinding,
}

#[derive(Clone, Debug)]
pub(crate) struct ShortcutRegistry {
    bindings: Vec<(ShortcutAction, Vec<ShortcutBinding>)>,
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self {
            bindings: ShortcutAction::ALL
                .into_iter()
                .map(|action| (action, action.default_bindings()))
                .collect(),
        }
    }
}

impl ShortcutRegistry {
    pub(crate) fn from_preferences(preferences: &[ShortcutPreference]) -> Self {
        let mut registry = Self::default();
        let mut decoded = Vec::new();
        for preference in preferences {
            let Some(action) = ShortcutAction::from_id(preference.action_id) else {
                continue;
            };
            let Ok(code) = serde_json::from_str::<Option<KeyCode>>(&preference.key_code) else {
                continue;
            };
            let Some(modifiers) = shortcut_modifiers_from_bits(preference.modifiers) else {
                continue;
            };
            if code.is_none() && !modifiers.is_empty() {
                continue;
            }
            let bindings = code
                .map(|code| ShortcutBinding::new(code, modifiers))
                .into_iter()
                .collect::<Vec<_>>();
            decoded.push((action, bindings));
        }
        for (action, bindings) in &decoded {
            if let Some((_, target)) = registry.bindings.iter_mut().find(|(id, _)| id == action) {
                *target = bindings.clone();
            }
        }
        if registry
            .bindings
            .iter()
            .any(|(action, bindings)| registry.validate_bindings(*action, bindings).is_err())
        {
            registry = Self::default();
            for (action, bindings) in decoded {
                let _ = registry.replace_bindings(action, bindings);
            }
        }
        registry
    }

    pub(crate) fn preferences(&self) -> Vec<ShortcutPreference> {
        self.bindings
            .iter()
            .filter(|(action, bindings)| *bindings != action.default_bindings())
            .filter_map(|(action, bindings)| {
                let binding = bindings.first();
                Some(ShortcutPreference {
                    action_id: *action as i32,
                    key_code: serde_json::to_string(&binding.map(|binding| binding.code)).ok()?,
                    modifiers: binding
                        .map_or(0, |binding| shortcut_modifiers_bits(binding.modifiers)),
                })
            })
            .collect()
    }

    pub(crate) fn action_for(&self, binding: ShortcutBinding) -> Option<ShortcutAction> {
        self.bindings
            .iter()
            .find_map(|(action, bindings)| bindings.contains(&binding).then_some(*action))
    }

    pub(crate) fn display_for(&self, action: ShortcutAction) -> String {
        self.bindings
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, bindings)| {
                if bindings.is_empty() {
                    "—".to_string()
                } else {
                    bindings
                        .iter()
                        .map(|binding| binding.display())
                        .collect::<Vec<_>>()
                        .join(" / ")
                }
            })
            .unwrap_or_else(|| "—".to_string())
    }

    #[cfg(test)]
    pub(crate) fn rebind(
        &mut self,
        action: ShortcutAction,
        binding: ShortcutBinding,
    ) -> Result<(), ShortcutConflict> {
        self.replace_bindings(action, vec![binding])
    }

    fn validate_bindings(
        &self,
        action: ShortcutAction,
        bindings: &[ShortcutBinding],
    ) -> Result<(), ShortcutConflict> {
        for &binding in bindings {
            if let Some(conflicting_action) = self.action_for(binding)
                && conflicting_action != action
            {
                return Err(ShortcutConflict {
                    action: conflicting_action,
                    binding,
                });
            }
        }
        Ok(())
    }

    fn replace_bindings(
        &mut self,
        action: ShortcutAction,
        replacement: Vec<ShortcutBinding>,
    ) -> Result<(), ShortcutConflict> {
        self.validate_bindings(action, &replacement)?;
        if let Some((_, bindings)) = self
            .bindings
            .iter_mut()
            .find(|(candidate, _)| *candidate == action)
        {
            *bindings = replacement;
        }
        Ok(())
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub(crate) fn transfer_bindings(
        &mut self,
        action: ShortcutAction,
        replacement: Vec<ShortcutBinding>,
    ) {
        self.transfer(action, replacement);
    }

    fn transfer(&mut self, action: ShortcutAction, replacement: Vec<ShortcutBinding>) {
        for (owner, bindings) in &mut self.bindings {
            if *owner == action {
                *bindings = replacement.clone();
            } else if bindings.iter().any(|binding| replacement.contains(binding)) {
                bindings.clear();
            }
        }
    }

    pub(crate) fn transfer_notice(
        &self,
        action: ShortcutAction,
        replacement: &[ShortcutBinding],
    ) -> String {
        let owners = self
            .bindings
            .iter()
            .filter(|(owner, bindings)| {
                *owner != action && bindings.iter().any(|binding| replacement.contains(binding))
            })
            .map(|(owner, _)| format!("«{}»", owner.title()))
            .collect::<Vec<_>>();
        if owners.len() == 1 {
            "При переназначении эта команда останется без горячих клавиш.".to_owned()
        } else {
            format!(
                "При переназначении {} останутся без горячих клавиш.",
                owners.join(", ")
            )
        }
    }
}

#[derive(Default)]
pub(crate) struct ShortcutEdit {
    pending: Option<(ShortcutAction, Vec<ShortcutBinding>)>,
}

impl ShortcutEdit {
    pub(crate) fn discard(&mut self) {
        self.pending = None;
    }

    pub(crate) fn stage(
        &mut self,
        registry: &ShortcutRegistry,
        action: ShortcutAction,
        bindings: Vec<ShortcutBinding>,
    ) -> Result<(), ShortcutConflict> {
        let result = registry.validate_bindings(action, &bindings);
        self.pending = Some((action, bindings));
        result
    }

    pub(crate) fn save(&mut self, registry: &mut ShortcutRegistry, action: ShortcutAction) -> bool {
        let Some((pending_action, bindings)) = self.pending.take() else {
            return false;
        };
        pending_action == action && registry.replace_bindings(action, bindings).is_ok()
    }

    pub(crate) fn overwrite(
        &mut self,
        registry: &mut ShortcutRegistry,
        action: ShortcutAction,
    ) -> bool {
        let Some((pending_action, bindings)) = self.pending.take() else {
            return false;
        };
        if pending_action != action {
            return false;
        }
        registry.transfer(action, bindings);
        true
    }
}

const SHORTCUT_MODIFIER_CONTROL: u8 = 1 << 0;
const SHORTCUT_MODIFIER_SHIFT: u8 = 1 << 1;
const SHORTCUT_MODIFIER_ALT: u8 = 1 << 2;
const SHORTCUT_MODIFIER_SUPER: u8 = 1 << 3;
const SHORTCUT_MODIFIERS_KNOWN: u8 = SHORTCUT_MODIFIER_CONTROL
    | SHORTCUT_MODIFIER_SHIFT
    | SHORTCUT_MODIFIER_ALT
    | SHORTCUT_MODIFIER_SUPER;

fn shortcut_modifiers_bits(modifiers: ModifiersState) -> u8 {
    let mut bits = 0;
    if modifiers.control_key() {
        bits |= SHORTCUT_MODIFIER_CONTROL;
    }
    if modifiers.shift_key() {
        bits |= SHORTCUT_MODIFIER_SHIFT;
    }
    if modifiers.alt_key() {
        bits |= SHORTCUT_MODIFIER_ALT;
    }
    if modifiers.super_key() {
        bits |= SHORTCUT_MODIFIER_SUPER;
    }
    bits
}

fn shortcut_modifiers_from_bits(bits: u8) -> Option<ModifiersState> {
    if bits & !SHORTCUT_MODIFIERS_KNOWN != 0 {
        return None;
    }
    let mut modifiers = ModifiersState::empty();
    if bits & SHORTCUT_MODIFIER_CONTROL != 0 {
        modifiers |= ModifiersState::CONTROL;
    }
    if bits & SHORTCUT_MODIFIER_SHIFT != 0 {
        modifiers |= ModifiersState::SHIFT;
    }
    if bits & SHORTCUT_MODIFIER_ALT != 0 {
        modifiers |= ModifiersState::ALT;
    }
    if bits & SHORTCUT_MODIFIER_SUPER != 0 {
        modifiers |= ModifiersState::SUPER;
    }
    Some(modifiers)
}

fn key_code_label(code: KeyCode) -> String {
    match code {
        KeyCode::Space => "Space".to_string(),
        KeyCode::ArrowLeft => "←".to_string(),
        KeyCode::ArrowRight => "→".to_string(),
        KeyCode::ArrowUp => "↑".to_string(),
        KeyCode::ArrowDown => "↓".to_string(),
        KeyCode::Comma => ",".to_string(),
        KeyCode::Period => ".".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        other => {
            let name = format!("{other:?}");
            name.strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name)
                .to_string()
        }
    }
}

pub(crate) fn is_modifier_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}
