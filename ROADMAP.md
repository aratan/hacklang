# HackLang — Roadmap técnico

Este documento separa las capacidades existentes de las que faltan. Las propuestas son objetivos, no funcionalidades ya implementadas. La compatibilidad con la sintaxis legacy y el modo compacto experimental es un requisito: ninguna mejora debe cambiar silenciosamente el comportamiento existente.

## Objetivo prioritario: interpretación y ejecutables nativos

HackLang debe poder ejecutar un mismo programa mediante el intérprete Rust o mediante un ejecutable nativo generado desde el código fuente. La compilación nativa es ahora un objetivo principal del proyecto, no una optimización aplazada a una fase futura.

Contrato de CLI previsto:

```sh
hl run programa.hl
hl build programa.hl
hl build programa.hl -o programa
hl check programa.hl
./programa
```

`hl programa.hl` seguirá funcionando como alias del intérprete. El primer objetivo de compilación será Linux x86-64 usando GCC o Clang. ARM64, Windows y la compilación cruzada quedan para fases posteriores.

La arquitectura inicial mantiene un único front-end (lexer, parser, AST y análisis estático no bloqueante), el evaluador Rust como referencia semántica y un backend C que enlaza con un runtime nativo dinámico. No se añadirá una IR prematuramente: se reevaluará cuando optimizaciones o varios backends lo justifiquen.

La especificación completa del contrato semántico, runtime, backend y criterios de aceptación está en [docs/NATIVE_COMPILER_DESIGN.md](docs/NATIVE_COMPILER_DESIGN.md).

## Estado actual y límites conocidos

- Intérprete Rust: lexer → parser → AST → evaluador; no hay IR ni compilador nativo.
- El evaluador recorre recursivamente el AST y limita la profundidad de evaluación a 256 niveles; no es configurable y todavía no existe una pila de operandos propia.
- El parser limita a 256 niveles la profundidad de expresiones y sentencias anidadas para devolver errores controlados; no hay todavía un parser iterativo.
- Los entornos usan `HashMap`, pero los mapas del lenguaje son `Vec<(Value, Value)>`: buscar una clave es lineal.
- El AST y los valores numéricos usan `f64`; no existen enteros de ancho fijo ni bytes como tipo propio.
- La asignación indexada se limita a una variable directa (por ejemplo, `a[i] = x`); no se admite actualizar directamente un destino anidado como `a[0][1] = x`.
- El comprobador estático es inicial y no bloqueante. Usa `unknown` para llamadas, parámetros y expresiones cuya información aún no puede deducir.
- Los errores se propagan con `Result<T, String>` en Rust, pero el lenguaje todavía no ofrece un mecanismo propio y uniforme para capturarlos y recuperarse.
- No hay módulos/imports ni biblioteca estándar de sistema/red.
- No hay builtins de archivos (`read_file`, `write_file`) ni ejecución de procesos (`exec`). Los ejemplos históricos que los invocaban están aislados en `prototypes/unsupported/`; no son parte de la API disponible.
- El conjunto `examples/*.hl` contiene programas compatibles y CI los ejecuta después de compilar y pasar los tests Rust.

## Objetivo de diseño

HackLang es un lenguaje de programación de propósito general. Debe permitir construir programas, herramientas, bibliotecas y aplicaciones, además de automatización y scripting técnico. Pentesting es solo uno de sus posibles dominios de uso. Mantener un núcleo pequeño y portable; exponer capacidades del sistema a través de módulos estándar explícitos y APIs coherentes. No convertir el intérprete en una colección de primitivas específicas de cada plataforma.

## Plan por etapas

### P0 — Fundamentos y seguridad del runtime
- Completar source spans desde lexer a parser/AST y diagnósticos estructurados. El lexer ya genera spans Unicode-aware; falta propagarlos.
- Introducir errores tipados con código, fase, mensaje y ubicación; mantener mensajes legibles y salida JSON estable.
- Definir límites configurables de profundidad, pasos/instrucciones, memoria y tiempo para ejecutar scripts no confiables.
- Evaluador y parser: límites defensivos de 256 niveles ya implementados; añadir configuración, pruebas de estrés/fuzzing y evaluar una pila explícita/trampolín si las mediciones lo justifican.
- Añadir fuzzing para lexer/parser y pruebas de estrés adicionales para recursión, anidamiento y entradas malformadas. Ya hay tests de regresión para los límites del parser y evaluador.

### P1 — Tipos para sistemas y protocolos
- Añadir enteros con signo/sin signo de anchuras explícitas (8/16/32/64 bits) y conversiones comprobadas.
- Añadir `bytes`/buffers, literales hexadecimales y operaciones endian-aware.
- Definir reglas inequívocas para overflow, división, conversiones y mezcla de tipos.
- Mantener `f64` y su comportamiento legacy; no reinterpretar silenciosamente números existentes.

### P2 — Colecciones y mutabilidad
- Sustituir la representación interna de mapas por una tabla hash adecuada, conservando igualdad de claves y semántica observable.
- Implementar asignación a destinos indexados anidados, evaluando cada índice una sola vez.
- Definir iteración estable y comportamiento de mutación durante la iteración.
- Medir rendimiento con benchmarks antes y después.

### P3 — Biblioteca estándar
- Strings: `len(string)` ya implementado con conteo de caracteres Unicode; faltan slicing definido, búsqueda, split/join, trim y conversiones.
- Colecciones: `len(array)` y `len(map)` ya implementados; faltan más operaciones estándar.
- Codificación: hex, Base64 y UTF-8 con errores explícitos.
- Regex con motor seguro frente a patrones patológicos (preferiblemente sin backtracking exponencial).
- Hashing y checksums: SHA-256 y algoritmos adicionales según necesidad; distinguir hashes de contraseñas de hashes rápidos.
- JSON: parseo, serialización y acceso seguro a campos.
- Pruebas de interoperabilidad y límites de tamaño para entradas externas.

### P4 — E/S y capacidades del sistema

Estado: pendiente. No asumir que `read_file()`, `write_file()` o `exec()` existen. Los prototipos históricos no son pruebas de funcionalidad.

- Archivos/directorios: lectura, escritura, listado, metadatos y rutas.
- Variables de entorno y argumentos de proceso.
- Procesos: ejecutar, capturar stdout/stderr, códigos de salida y terminación controlada.
- Permisos/capabilities: poder ejecutar en modo restringido sin acceso a filesystem, procesos o red salvo autorización explícita.
- Errores de E/S tipados; evitar exponer secretos en mensajes y logs.

### P5 — Redes para automatización y pentesting autorizado
- DNS y resolución.
- TCP/UDP y sockets con timeouts, límites y cierre determinista.
- HTTP/HTTPS: métodos, headers, cuerpos, TLS y gestión de certificados.
- APIs de bajo nivel para protocolos binarios sobre `bytes`.
- Límites de concurrencia, tamaño, tiempo y conexiones; no incluir escaneo agresivo por defecto.

### P6 — Módulos, paquetes y errores del lenguaje
- Sistema de módulos con imports explícitos, resolución determinista y control de versiones.
- Gestor de paquetes reproducible, manifiesto del proyecto, dependencias fijadas y lockfile.
- Separar biblioteca estándar y módulos opcionales.
- Diseñar manejo de errores recuperables: resultado explícito (`Result`/operador de propagación) como base; excepciones tipo `try/catch` solo si aportan una semántica clara.
- No ocultar errores ni convertir fallos en valores ambiguos.

### P7 — Concurrencia y asincronía
- Definir un modelo de concurrencia seguro y comprensible (tareas, async/await o alternativa justificada).
- Primitivas de sincronización y canales; evitar introducir carreras de datos por defecto.
- Cancelación, timeouts, propagación de errores y límites de recursos.
- No añadir concurrencia hasta que el runtime y el modelo de memoria tengan contratos claros.

### P8 — Tipado e inferencia
- Inferir tipos de parámetros a partir de usos y llamadas cuando sea seguro.
- Propagar tipos a través de ramas, retornos, colecciones, índices y funciones.
- Añadir anotaciones opcionales para parámetros/retornos y genéricos donde aporten valor.
- Mantener `unknown` como escape conservador cuando no haya evidencia suficiente; no inventar tipos.

### P9 — Compilador nativo y conformidad (prioridad actual)

1. **N0 — Contrato semántico:** auditar AST y evaluador; fijar truthiness, operadores, coerciones, scopes, closures, colecciones, impresión, errores y límites mediante tests.
2. **N1 — CLI/toolchain:** añadir `run`, `build`, `check`, alias legacy, detección de GCC/Clang y códigos de salida estables.
3. **N2 — Runtime C:** implementar valores etiquetados, strings, colecciones, entornos, funciones, flujo de control, errores y gestión de memoria.
4. **N3 — Backend por subconjuntos:** generar C desde AST, primero primitivas y control de flujo; después colecciones y funciones/closures.
5. **N4 — Build y diferencial:** enlazar el runtime, generar ejecutable y comparar stdout, stderr, código de salida y errores entre intérprete y binario nativo.
6. **N5 — Cobertura:** ampliar hasta cubrir todo el AST y la semántica del intérprete; solo entonces declarar paridad completa y abordar ARM64/Windows.

No reutilizar `prototypes/output.c` como backend funcional. No hacer obligatorio el checker estático para compilar. No declarar una característica implementada hasta tener tests y CI que la demuestren.

### P10 — Rendimiento y optimización
- Crear benchmarks reproducibles: tiempo, memoria, profundidad máxima y coste de colecciones.
- Medir coste real de tokens con tokenizadores representativos y comparar legibilidad/tasa de generación correcta (Pass@1).
- Evaluar IR, bytecode, optimizaciones o backends alternativos cuando benchmarks o requisitos concretos lo justifiquen.
- Mantener CI con tests, lint, formato, fuzz smoke tests y benchmarks no bloqueantes.

## Política de ejemplos y compatibilidad

- `examples/` contiene exclusivamente programas que deben funcionar con la versión actual de `hl`.
- Cada archivo `examples/*.hl` se ejecuta en CI; un ejemplo que no pase debe corregirse o trasladarse a `prototypes/unsupported/` con una explicación de la dependencia pendiente.
- Los ejemplos no pueden documentar funciones de runtime que no estén implementadas.
- Los cambios de gramática deben conservar la sintaxis legacy y probar por separado el modo compacto experimental.
- Todo ejemplo que pase en el intérprete y pertenezca al subconjunto nativo declarado debe pasar también en el ejecutable compilado.

## Criterios para aceptar cada capacidad

1. Pruebas unitarias y de integración, incluidos errores y límites.
2. Documentación con ejemplos ejecutables.
3. Sin regresiones en sintaxis legacy ni en modo compacto.
4. Medición cuando se alegue mejora de rendimiento o ahorro de tokens.
5. Límites seguros por defecto para operaciones de sistema y red.
6. Pruebas diferenciales intérprete/nativo para cada semántica cubierta.
