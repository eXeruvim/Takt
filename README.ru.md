<p align="center">
  <img src="assets/icons/takt-256.png" width="112" alt="Иконка Takt">
</p>

<h1 align="center">Takt</h1>

<p align="center">Локальный видеоплеер и аудиоплеер на Rust, Slint и libmpv.</p>

<p align="center"><a href="README.md">English</a> · <strong>Русский</strong></p>

Сейчас Takt поддерживает 64-битные Windows 10 и 11. Версия для Linux
запланирована после выпуска 1.0.

Текущая версия: `1.0.0-rc.3`.

## Возможности

- Воспроизведение видео и аудио, смешанная очередь файлов из папки.
- Аудиотеги, встроенные обложки, lossless-форматы и gapless-переходы.
- Выбор аудио- и субтитровых дорожек, внешние субтитры и главы.
- Скорость воспроизведения, эквалайзер, настройка соотношения сторон и
  необязательное аппаратное декодирование.
- Полноэкранный режим и окно в окне.
- Переназначаемые физические горячие клавиши, не зависящие от раскладки.
- Тёмная и светлая темы с выбором акцентного цвета.
- Необязательная конвертация и remux через FFmpeg.
- Подписанные обновления внутри приложения с автоматическим откатом.

## Установка

Скачайте установщик или portable-архив на странице Releases в GitHub.

Установщик ставит Takt для текущего пользователя и может добавить поддерживаемые
форматы в меню «Открыть с помощью». Portable-версию можно распаковать в любую
доступную для записи папку; `takt.exe` и `libmpv-2.dll` должны оставаться рядом.

Для воспроизведения не требуется системная установка mpv или FFmpeg. Конвертер
необязателен и использует пользовательский `ffmpeg.exe` из `PATH`, папки с
`takt.exe`, подпапки `tools` или пути в переменной `TAKT_FFMPEG`.

## Сборка из исходников

Требования:

- 64-битная Windows 10 или 11;
- стабильный Rust с target `x86_64-pc-windows-msvc`;
- Visual Studio Build Tools с инструментами C++;
- PowerShell 7.

Подготовьте закреплённый native runtime и запустите Takt:

```powershell
./scripts/fetch-native-runtime.ps1
cargo run --locked --features windows-gui
```

Файл или папку можно передать при запуске:

```powershell
cargo run --locked --features windows-gui -- "C:\Медиа\видео.mp4"
target\debug\takt.exe "C:\Музыка\Альбом"
```

Сборка пакетов для распространения:

```powershell
./scripts/build-release.ps1
./scripts/build-installer.ps1
```

Готовые файлы появятся в `artifacts/release`.

## Проверки

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked -p libmpv --lib ownership_tests
```

## Документация

- [Архитектура](ARCHITECTURE.md) · [English](ARCHITECTURE.en.md)
- [Бенчмарки](BENCHMARKING.md) · [English](BENCHMARKING.en.md)
- [Поддержка и известные ограничения](SUPPORT.md) · [English](SUPPORT.en.md)
- [Матрица кодеков](CODEC_MATRIX.md)
- [Уведомления о сторонних компонентах](THIRD_PARTY_NOTICES.md)

## Лицензия

Takt распространяется по лицензии [GNU General Public License v3.0 или новее](LICENSE).
