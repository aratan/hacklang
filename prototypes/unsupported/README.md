# Ejemplos pendientes de implementación

Los archivos `.hl` de este directorio son prototipos históricos. No pertenecen al conjunto de ejemplos ejecutables ni deben incluirse en pruebas de compatibilidad de la versión actual.

| Archivo | Capacidad pendiente | Estado |
|---|---|---|
| `test_io.hl` | `read_file()` y `write_file()` | No implementadas en el runtime |
| `test_exec.hl` | `exec()` | No implementada; requiere además un modelo explícito de permisos y aislamiento |

No se deben simular estas funciones ni asumir que existen por aparecer en ejemplos antiguos. Cuando se implementen, deberán tener pruebas de integración, documentación y controles de seguridad. Los ejemplos compatibles se mantienen en `examples/` y CI los ejecuta automáticamente.
