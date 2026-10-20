# Takt fuzz targets

Этот пакет изолирован от обычной сборки Takt и предназначен только для
продолжительных проверок парсеров. `cargo-fuzz` использует nightly-компилятор и
libFuzzer. На Windows требуются MSVC v143 x64/x86 build tools и x64 runtime
AddressSanitizer из компонента C++ AddressSanitizer или LLVM.

```powershell
rustup toolchain install nightly
cargo +nightly install cargo-fuzz --locked
.\scripts\run-fuzz.ps1 -Target audio_metadata -DurationSeconds 300
.\scripts\run-fuzz.ps1 -Target artwork -DurationSeconds 300
```

Для длительного локального прогона задайте нужное значение `DurationSeconds`.
Скрипт сам находит MSVC linker и ASan runtime и не меняет системное окружение.
Найденные входы появятся в `fuzz/artifacts/`; перед добавлением регрессионного
fixture минимизируйте их командой `cargo fuzz tmin` и убедитесь, что файл не
содержит чужих медиаданных.
