# Oxide fuzz targets

Этот пакет изолирован от обычной сборки Oxide и предназначен только для
продолжительных проверок парсеров. `cargo-fuzz` использует nightly-компилятор и
libFuzzer; официальный инструмент запускается на Unix-подобной системе. На
Windows используйте WSL.

```bash
rustup toolchain install nightly
cargo install cargo-fuzz --locked
cargo +nightly fuzz run audio_metadata -- -max_total_time=300
cargo +nightly fuzz run artwork -- -max_total_time=300
```

Для длительного локального прогона уберите `-max_total_time` либо задайте нужное
число секунд. Найденные входы появятся в `fuzz/artifacts/`; перед добавлением
регрессионного fixture минимизируйте их командой `cargo fuzz tmin` и убедитесь,
что файл не содержит чужих медиаданных.
