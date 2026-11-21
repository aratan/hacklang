# Prototipos no integrados

Este directorio conserva material experimental que no forma parte de la versión ejecutable de HackLang.

- `output.c`: prototipo C histórico. No forma parte del intérprete Rust, no lo genera `hl` y no está conectado a un backend de compilación. No debe tratarse como salida generada ni como compilador funcional.
- `unsupported/`: ejemplos `.hl` históricos que dependen de capacidades aún no implementadas. Consulta su README antes de utilizarlos.

Los programas compatibles con la versión actual deben estar en `examples/` y ejecutarse mediante `hl` (o `cargo run -- ...` durante el desarrollo).