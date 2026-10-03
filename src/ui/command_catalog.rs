pub(crate) struct CommandSpec {
    pub(crate) action: i32,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) shortcut: &'static str,
    keywords: &'static str,
    pub(crate) requires_media: bool,
}

pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        action: 0,
        title: "Открыть файл",
        description: "Выбрать видео или аудиофайл",
        shortcut: "Ctrl O",
        keywords: "open media video audio открыть медиа видео аудио",
        requires_media: false,
    },
    CommandSpec {
        action: 1,
        title: "Открыть папку",
        description: "Собрать плейлист из папки",
        shortcut: "Ctrl Shift O",
        keywords: "open folder directory playlist открыть каталог плейлист",
        requires_media: false,
    },
    CommandSpec {
        action: 24,
        title: "Меню команд",
        description: "Открыть поиск по действиям плеера",
        shortcut: "Ctrl K",
        keywords: "command palette menu поиск команды палитра меню",
        requires_media: false,
    },
    CommandSpec {
        action: 2,
        title: "Пауза / воспроизведение",
        description: "Переключить состояние плеера",
        shortcut: "Space",
        keywords: "play pause start stop играть пауза воспроизведение",
        requires_media: true,
    },
    CommandSpec {
        action: 3,
        title: "Предыдущий файл",
        description: "Перейти назад по плейлисту",
        shortcut: "P",
        keywords: "previous back playlist предыдущий назад",
        requires_media: true,
    },
    CommandSpec {
        action: 4,
        title: "Следующий файл",
        description: "Перейти вперёд по плейлисту",
        shortcut: "N",
        keywords: "next forward playlist следующий вперед вперёд",
        requires_media: true,
    },
    CommandSpec {
        action: 22,
        title: "Назад на 5 секунд",
        description: "Короткая перемотка текущего файла назад",
        shortcut: "←",
        keywords: "seek short rewind back короткая перемотка назад",
        requires_media: true,
    },
    CommandSpec {
        action: 23,
        title: "Вперёд на 5 секунд",
        description: "Короткая перемотка текущего файла вперёд",
        shortcut: "→",
        keywords: "seek short forward короткая перемотка вперед вперёд",
        requires_media: true,
    },
    CommandSpec {
        action: 13,
        title: "Назад на 10 секунд",
        description: "Перемотать текущий файл назад",
        shortcut: "J",
        keywords: "seek rewind back перемотка назад",
        requires_media: true,
    },
    CommandSpec {
        action: 14,
        title: "Вперёд на 10 секунд",
        description: "Перемотать текущий файл вперёд",
        shortcut: "L",
        keywords: "seek forward перемотка вперед вперёд",
        requires_media: true,
    },
    CommandSpec {
        action: 25,
        title: "Предыдущий кадр",
        description: "Поставить видео на паузу и перейти на кадр назад",
        shortcut: ",",
        keywords: "frame back previous кадр назад предыдущий покадрово",
        requires_media: true,
    },
    CommandSpec {
        action: 26,
        title: "Следующий кадр",
        description: "Поставить видео на паузу и перейти на кадр вперёд",
        shortcut: ".",
        keywords: "frame forward next кадр вперед вперёд следующий покадрово",
        requires_media: true,
    },
    CommandSpec {
        action: 5,
        title: "Выключить / включить звук",
        description: "Переключить mute",
        shortcut: "M",
        keywords: "mute unmute sound audio звук без звука",
        requires_media: true,
    },
    CommandSpec {
        action: 15,
        title: "Уменьшить громкость",
        description: "Убавить громкость на 5%",
        shortcut: "↓",
        keywords: "volume down sound громкость тише убавить",
        requires_media: true,
    },
    CommandSpec {
        action: 16,
        title: "Увеличить громкость",
        description: "Прибавить громкость на 5%",
        shortcut: "↑",
        keywords: "volume up sound громкость громче прибавить",
        requires_media: true,
    },
    CommandSpec {
        action: 6,
        title: "Повтор файла",
        description: "Включить или выключить повтор",
        shortcut: "R",
        keywords: "repeat loop повтор цикл",
        requires_media: true,
    },
    CommandSpec {
        action: 7,
        title: "Показать плейлист",
        description: "Открыть или закрыть очередь файлов",
        shortcut: "Ctrl L",
        keywords: "playlist queue список очередь файлы",
        requires_media: true,
    },
    CommandSpec {
        action: 8,
        title: "Свойства файла",
        description: "Показать информацию о медиа",
        shortcut: "I",
        keywords: "info properties metadata свойства информация метаданные",
        requires_media: true,
    },
    CommandSpec {
        action: 9,
        title: "Настройки воспроизведения",
        description: "Скорость, повтор папки и перемешивание",
        shortcut: "—",
        keywords: "settings speed shuffle настройки скорость перемешивание",
        requires_media: false,
    },
    CommandSpec {
        action: 11,
        title: "Полноэкранный режим",
        description: "Переключить fullscreen",
        shortcut: "F",
        keywords: "fullscreen full screen полный экран",
        requires_media: false,
    },
    CommandSpec {
        action: 27,
        title: "Картинка в картинке",
        description: "Переключить компактное окно поверх остальных",
        shortcut: "Alt P",
        keywords: "picture in picture pip mini player окно поверх компактное",
        requires_media: true,
    },
    CommandSpec {
        action: 12,
        title: "Остановить воспроизведение",
        description: "Закрыть текущий файл",
        shortcut: "S",
        keywords: "stop close остановить закрыть",
        requires_media: true,
    },
    CommandSpec {
        action: 28,
        title: "Конвертировать файл",
        description: "Создать копию в другом формате через FFmpeg",
        shortcut: "—",
        keywords: "convert export transcode ffmpeg конвертация экспорт формат кодек",
        requires_media: true,
    },
    CommandSpec {
        action: 29,
        title: "Сбросить горячие клавиши",
        description: "Вернуть стандартные сочетания",
        shortcut: "—",
        keywords: "reset defaults shortcuts сбросить клавиши сочетания по умолчанию",
        requires_media: false,
    },
];

pub(crate) fn search(query: &str) -> Vec<&'static CommandSpec> {
    let words = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    COMMANDS
        .iter()
        .filter(|command| {
            let searchable = format!(
                "{} {} {}",
                command.title, command.description, command.keywords
            )
            .to_lowercase();
            words.iter().all(|word| searchable.contains(word))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_complete_catalog_in_display_order() {
        let found = search("");
        assert_eq!(found.len(), COMMANDS.len());
        assert_eq!(found.first().map(|command| command.action), Some(0));
        assert_eq!(found.last().map(|command| command.action), Some(29));
    }

    #[test]
    fn search_matches_every_word_across_localized_metadata() {
        let found = search("video открыть");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].action, 0);

        let found = search("метаданные info");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].action, 8);
    }
}
