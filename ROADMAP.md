# HackLang — Roadmap técnico

Este documento separa las capacidades existentes de las que faltan. Las propuestas son objetivos, no funcionalidades ya implementadas. La compatibilidad con la sintaxis legacy y el modo compacto experimental es un requisito: ninguna mejora debe cambiar silenciosamente el comportamiento existente.

## Protocolo de continuidad del proyecto

Antes de cada intervención de código, sincronizar y revisar obligatoriamente:

1. HEAD de la rama de trabajo, relación con `main`, commits recientes y diff completo de la PR activa.
2. Descripción actual de la PR, archivos cambiados, comentarios, revisiones y estado de CI.
3. Issues abiertas y cerradas relacionadas con el componente; no inferir pendientes solo por memoria conversacional.
4. Roadmap y documentos de diseño pertinentes, contrastándolos con el código y sus pruebas actuales.
5. Implementación y regresiones existentes antes de diseñar o duplicar una solución.

Reglas de ejecución:

- Un hito con implementación, pruebas relevantes y CI satisfactoria se considera completado para el subconjunto declarado. No se vuelve a implementar ni a planificar como pendiente sin una regresión reproducible o un requisito nuevo.
- Los documentos históricos y los resúmenes de PR pueden quedar obsoletos: prevalecen el HEAD, el código y las pruebas actuales. Corregir la documentación cuando haya divergencias.
- Cada intervención debe resolver un objetivo acotado, añadir o ampliar una prueba que lo demuestre y verificar CI. No encadenar cambios nuevos sobre una ejecución fallida sin corregir primero su causa.
- Al terminar, actualizar este roadmap y la descripción de la PR con commit, evidencia de pruebas, límites restantes y siguiente tarea concreta.
- Mantener una única fuente de verdad para el siguiente paso; no reabrir trabajo ya validado por inercia o por resúmenes antiguos.

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

- Intérprete Rust y compilador nativo C comparten lexer → parser → AST. `hl build` ya genera ejecutables para un subconjunto explícito; no hay IR adicional.
- El evaluador recorre recursivamente el AST y limita la profundidad de evaluación a 256 niveles, configurable mediante CLI; también aplica un presupuesto configurable de instrucciones por ejecución. Todavía no existe una pila de operandos propia.
- El parser limita a 256 niveles la profundidad de expresiones y sentencias anidadas para devolver errores controlados; no hay todavía un parser iterativo.
- Los entornos usan `HashMap`, pero los mapas del lenguaje son `Vec<(Value, Value)>`: buscar una clave es lineal.
- El AST y los valores numéricos usan `f64`; no existen enteros de ancho fijo ni bytes como tipo propio.
- La asignación indexada parte de un binding raíz; el HIR y el runtime nativo admiten rutas anidadas como `a[0][1] = x`. Los mapas siguen usando pares ordenados y búsqueda lineal.
- El comprobador estático es inicial y no bloqueante. Usa `unknown` para llamadas, parámetros y expresiones cuya información aún no puede deducir.
- Los errores se propagan con `Result<T, String>` en Rust, pero el lenguaje todavía no ofrece un mecanismo propio y uniforme para capturarlos y recuperarse.
- No hay módulos/imports ni biblioteca estándar de sistema/red.
- No hay builtins de archivos (`read_file`, `write_file`) ni ejecución de procesos (`exec`). Los ejemplos históricos que los invocaban están aislados en `prototypes/unsupported/`; no son parte de la API disponible.
- El conjunto `examples/*.hl` contiene programas compatibles y CI los ejecuta después de compilar y pasar los tests Rust.

## Objetivo de diseño

HackLang es un lenguaje de programación de propósito general. Debe permitir construir programas, herramientas, bibliotecas y aplicaciones, además de automatización y scripting técnico. Pentesting es solo uno de sus posibles dominios de uso. Mantener un núcleo pequeño y portable; exponer capacidades del sistema a través de módulos estándar explícitos y APIs coherentes. No convertir el intérprete en una colección de primitivas específicas de cada plataforma.

## Plan por etapas

### P0 — Fundamentos y seguridad del runtime
- Source Spans: propagación implementada y fusionada mediante PR #18 (lexer, parser, sidecar AST, checker y errores AST→HIR). CI #1211 validó el HEAD final fusionado: type-check, build, tests unitarios, ejemplos y comparación intérprete/nativo. La ruta de compilación nativa también conserva los rangos de errores AST→HIR (regresión añadida en `native.rs`). Los errores actuales de `capture::analyze`, `plan_storage` y validadores de `memory` son fallos de invariantes internas sobre IDs de HIR, no diagnósticos atribuibles a una construcción fuente; se dejan sin rango deliberadamente. Los mensajes del compilador C describen el C generado y no se presentan como ubicaciones HackLang. La provenance de traps del runtime nativo se añadió después mediante PR #30: el HIR/runtime C conserva rangos de sentencia y los diagnósticos JSON los incluyen, también en llamadas anidadas.
- Diagnósticos tipados: PR #24 cubre lexer/parser/checker; PR #25 cubre HIR/lowering y backend nativo; PR #26 añade fase runtime al evaluador Rust; PR #27 añade salida JSON opt-in para traps C mediante `HACKLANG_DIAGNOSTICS=json`; PR #28 añade rangos de sentencia a errores del evaluador Rust usando el sidecar AST. Los traps C también incluyen rangos de sentencia desde PR #30; no confundirlos con ubicaciones de errores internos del compilador sin nodo fuente.
- Límites configurables de profundidad: implementados en PR #20 para parser y evaluador, con valor predeterminado compatible de 256 y opciones CLI validadas. Los presupuestos de instrucciones, memoria y tiempo del runtime nativo se completaron en PR #32–#35; el intérprete Rust todavía no tiene límites propios de memoria ni de tiempo.
- Evaluador y parser: límites defensivos y configurables de 256 niveles ya implementados; ampliar pruebas de estrés/fuzzing y evaluar una pila explícita/trampolín solo si las mediciones lo justifican.
- Fuzz-smoke determinista del lexer/parser: PR #22 fusionada; corpus de regresión, 256 entradas sintéticas acotadas, ambos modos de gramática y límite de profundidad. PR #23 fusionada añade targets libFuzzer separados para lexer y parser, corpus semilla y workflow con campañas cortas en PR y campañas programadas/manuales. Los smoke campaigns #1 y #2 completaron ambos targets; no declarar ausencia de bugs por una ejecución corta.

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

Estado real: el backend C ya ejecuta un subconjunto amplio del lenguaje, incluidas funciones y closures. La CI valida type-check, build, tests unitarios, ejemplos, paridad diferencial y pruebas ASan/UBSan que compilan y ejecutan programas completos generados desde HIR. No hay paridad completa; quedan por ampliar escapes, errores y stress tests.

1. **N0 — Contrato semántico (parcial):** lexer/parser con spans, sintaxis legacy y compacta, errores y límites defensivos. Seguir ampliando Unicode, impresión, límites y casos de borde.
2. **N1 — CLI/toolchain (implementado):** `run`, `build`, `check`, alias legacy, selección GCC/Clang y protección frente a sobrescritura accidental.
3. **N2 — Runtime C (operativo para el subconjunto probado):** valores etiquetados Number/Bool/String/Array/Map/Closure; entornos, operadores, truthiness, indexación, asignación indexada, indexación Unicode de strings y `len`. Arenas bump por lane/ámbito, slots `HlFrame`, almacenamiento por binding, limpieza en cierres normales y errores fatales, y tracing collector para ciclos de closures/celdas. Límites por ejecución: 1.000.000 instrucciones, 256 MiB de reservas de arenas, 256 MiB de heap auxiliar y 30 segundos de CPU por defecto; configurables por variables de entorno. Los dos presupuestos de memoria son independientes y no equivalen a un límite absoluto de RSS.
4. **N3 — Backend HIR→C (funciones habilitadas):** cuerpos de expresión/bloque, parámetros, `return`, frames de activación, llamadas directas y dinámicas, capturas por celdas mutables compartidas, llamadas de orden superior, recursión y recursión mutua. HIR mantiene IDs resueltos y análisis determinista de capturas; no se ha duplicado el frontend.
5. **N4 — Paridad diferencial (en expansión):** CI compara intérprete y ejecutable nativo. `tests/native_array_parity.rs` cubre índices fuera de rango, negativos, fraccionarios, tipos inválidos, límites de `size_t`, rutas anidadas, errores de mapas/cadenas y cadenas con NUL embebido. PR #41 (`5c00eaa3`, CI #1402) hace que el runtime nativo preserve la longitud explícita de las cadenas y mantiene paridad de `len`, indexación, truthiness, claves de mapas, clonación, concatenación e impresión con U+0000. `src/native.rs` incluye casos ASan/UBSan de indexación Unicode, fallos dentro de closures recursivas y unwinding con capturas anidadas mutadas; `tests/native_cell_lifetime.rs` comprueba limpieza LIFO exactamente una vez. Issues #11 (paridad de errores/índices/unwinding) y #12 (stress de closures/colecciones/recursos) están cerradas con pruebas en el código. No reimplementar estos hitos; ampliar solo ante una brecha concreta o regresión reproducible.
6. **N5 — Memoria y vida útil de valores (operativa para el subconjunto nativo actual):** `src/memory.rs` calcula y valida CFG/liveness, interferencias, slots, lanes, promociones y planes de activación. El runtime C usa celdas estables para capturas y un tracing collector especializado para ciclos del grafo de celdas/closures, con raíces de frames activos y temporales de expresión. Los temporales C se evalúan en orden explícito y se registran como raíces hasta terminar su uso. Los valores retornados desde bloques internos se promueven al entorno de activación antes de liberar regiones léxicas. Las lanes físicas se restauran en cierres normales, `break`, `continue` y errores. Arrays/mapas mantienen semántica de valor y sus grafos se copian a almacenamiento propiedad del binding; las celdas capturadas mantienen identidad compartida y disponen de lanes privadas para sus grafos actuales, liberadas al sobrescribirlas o recoger la celda. La suite diferencial y ASan/UBSan cubre closures que escapan, grafos anidados, recursión, raíces temporales y unwinding. No es un GC general para todos los objetos ni una optimización de escape global; el planner conserva decisiones conservadoras. No reabrir este hito salvo una regresión reproducible o un caso de vida útil no cubierto.

**Estado actualizado (2026-11-26):** issues #11, #12 y #13 están cerradas. La PR #17 corrigió el entorno léxico por iteración en el intérprete y validó 96 closures escapadas, reutilización de frames y guardrails existentes. La PR #16 se cerró como sustituida tras comparar su escenario con #17: aportaba una ruta distinta (closures devueltas por activaciones de una fábrica y almacenadas en mapas), que se conservó como prueba complementaria en la PR #19. La PR #18 integró Source Spans. La PR #19 añadió esa prueba diferencial complementaria y se fusionó mediante squash en el commit `754ee77232f6f4f42beea045b09d937ee771d3a6`; su CI #1214 terminó completamente verde. PR #20 se fusionó en `72ba2f29ddba4af92fd2caddceceea90b1317d50` (CI #1226 verde); PR #22 se fusionó en `904324ba3a27acd75dfeae40c6ce4e04cf163744` (CI #1234 verde); PR #23 se fusionó en `5b2a8bb1ae340d78c4cd215af18fdd62148ecd81` (Rust CI #1238 y fuzz campaigns #1/#2 verdes). PR #24 se fusionó en `913b1db6f7e5e7d43349da8b95baffd52db1a1ba` (CI #1255 verde); PR #25 en `0f8e40593be7c7c6e55576cb321415a02ca18831` (CI #1267 verde); PR #26 en `973c82985930914e70d51fc9febbeecb2d35aadb` (CI #1276 verde); PR #27 en `470537a8005d1089155a3e5235e895a53d9e2166` (CI #1289 verde); PR #28 en `82b7d0b719efb11df11d6db5290381a4faf65bac` (CI #1302 verde). `main` contiene estas integraciones.

**Estado actualizado (2026-11-26, revisión tras PR #30):** PR #30 está fusionada en `main` mediante el commit `3983f3016e4907ecc4908839adf7c1f37fc8173e`. Añade provenance de sentencia al HIR/runtime C y rangos en diagnósticos JSON de traps nativos, incluidos los anidados. CI #1312 y #1313 terminaron en verde (type-check, build, tests, ejemplos, paridad y sanitizers). Por tanto, provenance nativa ya no es una tarea pendiente.

**Seguridad del runtime — estado verificado (2026-11-26):** PR #31 (`d95201a8`) añade el presupuesto de instrucciones al intérprete Rust; PR #32 (`da38ed2c`) lo porta al runtime C; PR #33 (`190f8aa9`) limita reservas de arenas nativas; PR #34 (`f6eee78c`) contabiliza memoria auxiliar viva, incluidas liberaciones y `realloc`; PR #35 (`94dc0894`) añade 30 segundos de tiempo de CPU por defecto al runtime nativo. PR #37 (`2023d07b`, CI #1365) estresa el allocator auxiliar y PR #38 (`61284ecb`, CI #1367) prueba buffers reales de cadenas. PR #39 (`567c4a57`, CI #1370) verifica que el límite temporal nativo también actúa en llamadas anidadas. PR #40 (`dfd50a6d`, CI #1394) añade al intérprete Rust un presupuesto temporal cooperativo predeterminado de 30 segundos, configurable por CLI y entorno, y comprueba los presupuestos de tiempo e instrucciones en cada iteración, incluso con cuerpo vacío. CI verde: type-check, build, 332+ tests, ejemplos y comparación intérprete/nativo. Los límites de memoria del runtime C son independientes y no representan un límite absoluto de RSS. Los límites temporales son cooperativos y no interrumpen operaciones bloqueantes.

**Auditoría de salida — estado parcial (2026-11-27):** PR #42 corrige dos divergencias reproducibles encontradas por pruebas diferenciales. En `src/hir_native.rs`, los literales numéricos del C generado pasan a notación científica con 17 cifras decimales, evitando que valores finitos pequeños como `1e-300` se conviertan en cero por el formato fijo anterior. En `src/native_runtime.c`, la impresión de depuración decodifica UTF-8 y escapa controles Unicode C0/C1 (incluidos DEL y U+0085) como Rust Debug. Las pruebas cubren subnormales, valores grandes, límites de notación, precisión, cero negativo, escalares Unicode, marcas combinantes/ZWJ en indexación y controles C1 en depuración. PR #42 se fusionó mediante squash en `554688e2bfce51cbabbc1de89916b7cd5aa0d07d`. CI #1407 del HEAD final terminó verde: type-check, build, 333 tests unitarios, 32 pruebas diferenciales, ejemplos y comparación intérprete/nativo. PR #43 (`69cd559272dee5468acf5b618cbab91c62b70182`) completa el siguiente subconjunto: el runtime C clasifica puntos de código mediante rangos Unicode independientes de locale y escapa categorías no imprimibles (Mn/Me, Cf, Cc, Cs, Co, Cn, Zl/Zp). Añade pruebas diferenciales para marcas combinantes, ZWJ, soft hyphen y separadores de línea/párrafo. CI #1408 terminó verde: type-check, build, tests unitarios, ejemplos y comparación intérprete/nativo. Esto amplía la paridad de depuración, pero no demuestra por sí solo conformidad Unicode total: la tabla generada debe mantenerse alineada con la versión Unicode de Rust y cubrirse con pruebas adicionales al actualizar toolchains. No reabrir memoria/closures.

PR #44 (`9cdb9d46f3255f8fdf35cbefbf9c79e9fc62be1c`) añade una prueba diferencial de crecimiento repetido de cadenas: 4.096 concatenaciones con comprobación de longitud, último carácter y contenido completo. CI #1410 pasó los 10 pasos: type-check, build, tests unitarios, ejemplos y paridad intérprete/nativo. No cambia el runtime; verifica el allocator/camino de cadenas ya implementado. Mantener intactos los hitos validados de memoria, escapes y paridad de índices/unwinding.

**Siguiente prioridad:** probar la representación numérica de `NaN`, `+Infinity` y `-Infinity` producidos por operaciones aritméticas válidas, comparando intérprete y backend nativo. Después, ampliar casos límite de UTF-8 y revisar longitudes grandes sin crear asignaciones desproporcionadas.

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
