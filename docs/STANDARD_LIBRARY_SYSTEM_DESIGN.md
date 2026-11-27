# Diseño de la biblioteca estándar de sistema

Estado: especificación de implementación; las APIs descritas aquí no se consideran disponibles hasta que exista código, pruebas y CI verde.

## Principios

- El mismo programa debe conservar semántica entre `hl run` y `hl build`.
- El frontend único y el HIR son la frontera de compilación. Los builtins se reconocen explícitamente durante AST→HIR; no se simulan como variables globales ni closures.
- La biblioteca se organiza en módulos (`io`, `fs`, `net`) cuando exista sistema de imports. Hasta entonces, los nombres builtin deben llevar prefijo estable (`io_read_line`, `fs_read`, `net_tcp_connect`) para evitar colisiones con funciones del usuario.
- Las llamadas de sistema devuelven errores explícitos. EOF no puede confundirse con una línea vacía.
- Límites configurables de bytes, tiempo y conexiones; sin operaciones de red ilimitadas.
- El modo restringido debe poder deshabilitar filesystem y red. La política no puede diferir entre intérprete y ejecutable nativo.
- No modificar ni reabrir el modelo de memoria/escape analysis existente por esta funcionalidad.

## Primera entrega: consola

API inicial:
- `io_read_line()`: devuelve una línea sin terminador; EOF y errores se distinguen mediante error de runtime.
- `print expr;`: salida estándar existente, conservando el formato actual.
- Más adelante: salida de error y lectura de bytes.

Aceptación: calculadora interactiva de ejemplo, manejo de EOF, entrada vacía, CRLF, UTF-8 y líneas que exceden el límite; paridad intérprete/nativo.

## Segunda entrega: filesystem

API prevista:
- `fs_read(path)`: lee un fichero UTF-8 completo sujeto a `HACKLANG_MAX_FILE_BYTES`.
- `fs_write(path, content)`: escritura completa; no sobrescribe por defecto. Añadir una operación explícita para reemplazo atómico.
- `fs_exists(path)`, `fs_is_file(path)`, `fs_is_dir(path)`.
- `fs_list(path)`: listado ordenado y determinista.
- Posteriormente metadatos, creación de directorios y lectura/escritura binaria mediante `bytes`.

Las rutas relativas se resuelven desde el directorio de trabajo del proceso. Definir política de symlinks y sandbox antes de habilitar modo restringido. No prometer aislamiento de rutas solo con validación textual (`..` no es una defensa suficiente frente a symlinks y carreras).

## Tercera entrega: sockets

API TCP prevista:
- `net_tcp_connect(host, port, timeout_ms)`
- `net_send(socket, data)`
- `net_recv(socket, max_bytes, timeout_ms)`
- `net_close(socket)`

Contrato:
- Los handles de socket son recursos opacos, no números reutilizables por el usuario.
- Cierre determinista y también en errores.
- Límites de conexiones simultáneas, tamaño de payload y timeout.
- Recepción parcial es válida; TCP es un flujo, no conserva límites de mensajes.
- Errores DNS, conexión rechazada, timeout, EOF remoto y cierre local deben distinguirse.
- Los sockets no se habilitan por defecto en un modo sandbox/restringido.
- UDP, TLS, HTTP y servidores se implementan después de validar la API TCP.

## Arquitectura necesaria

1. Añadir un catálogo tipado de builtins al HIR, con validación de aridad en lowering.
2. Implementar cada operación en el evaluador Rust y en el runtime nativo C con el mismo contrato.
3. Definir propiedad y vida de handles de archivo/socket, incluyendo cierre por error.
4. Mantener errores y límites equivalentes entre motores.
5. Tests unitarios, integración, pruebas diferenciales y pruebas de estrés locales sin depender de servicios externos.
6. Documentar ejemplos ejecutables y actualizar `ROADMAP.md` solo al cerrar cada subconjunto.

## Compilación sin GCC/Clang

El backend actual genera C y llama a GCC/Clang; no es una dependencia accidental, sino parte explícita del diseño vigente. Eliminarla requiere un backend que emita código máquina u objetos.

Ruta recomendada:
1. Separar el contrato semántico del backend C y completar pruebas diferenciales.
2. Evaluar Cranelift como backend Rust nativo para x86-64/AArch64, reutilizando HIR y los análisis actuales. Esto elimina la compilación C externa para generar código, pero no resuelve por sí solo el enlazado final.
3. Emitir objetos mediante una biblioteca como `object` y enlazar con un linker disponible o distribuido con la toolchain.
4. Para ejecutables totalmente autónomos, incorporar runtime compilado al mismo backend y resolver libc/ABI, relocaciones, símbolos, TLS, syscalls y formatos ELF/PE/Mach-O. No prometer independencia completa del linker antes de validar esa etapa.
5. Mantener temporalmente backend C como fallback de compatibilidad; retirar GCC/Clang solo cuando CI demuestre paridad y builds reproducibles.

No introducir LLVM/Cranelift ni un linker propio antes de medir coste, madurez, licencia, tamaño y soporte real de targets. La primera meta verificable es generar un objeto nativo sin invocar un compilador C externo.
