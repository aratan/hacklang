# HackLang — Roadmap técnico

Este documento separa las capacidades existentes de las que faltan. Las propuestas son objetivos, no funcionalidades ya implementadas. La compatibilidad con la sintaxis legacy y el modo compacto experimental es un requisito: ninguna mejora debe cambiar silenciosamente el comportamiento existente.

## Estado actual y límites conocidos

- Intérprete Rust: lexer → parser → AST → evaluador; no hay IR ni compilador nativo.
- El evaluador recorre recursivamente el AST. La profundidad de expresiones, llamadas y recursión puede agotar la pila nativa; todavía no hay límite configurable ni evaluación con pila explícita.
- Los entornos usan `HashMap`, pero los mapas del lenguaje son `Vec<(Value, Value)>`: buscar una clave es lineal.
- El AST y los valores numéricos usan `f64`; no existen enteros de ancho fijo ni bytes como tipo propio.
- La asignación indexada se limita a una variable directa (por ejemplo, `a[i] = x`); no se admite actualizar directamente un destino anidado como `a[0][1] = x`.
- El comprobador estático es inicial y no bloqueante. Usa `unknown` para llamadas, parámetros y expresiones cuya información aún no puede deducir.
- Los errores se propagan con `Result<T, String>` en Rust, pero el lenguaje todavía no ofrece un mecanismo propio y uniforme para capturarlos y recuperarse.
- No hay módulos/imports ni biblioteca estándar de sistema/red.

## Objetivo de diseño

HackLang debe servir tanto para automatización general como para scripting técnico y pentesting autorizado. Mantener un núcleo pequeño y portable; exponer capacidades del sistema a través de módulos estándar explícitos, con permisos y límites configurables. No convertir el intérprete en una colección de primitivas específicas de cada plataforma.

## Plan por etapas

### P0 — Fundamentos y seguridad del runtime
- Completar source spans desde lexer a parser y diagnósticos estructurados.
- Introducir errores tipados con código, fase, mensaje y ubicación; mantener mensajes legibles y salida JSON estable.
- Definir límites configurables de profundidad, pasos/instrucciones, memoria y tiempo para ejecutar scripts no confiables.
- Evitar desbordamientos de pila: primero límites defensivos y pruebas de estrés; después evaluar un evaluador con pila explícita/trampolín.
- Añadir pruebas de fuzzing para lexer/parser y pruebas de estrés para recursión, anidamiento y entradas malformadas.

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
- Strings: longitud Unicode, slicing definido, búsqueda, split/join, trim y conversiones.
- Codificación: hex, Base64 y UTF-8 con errores explícitos.
- Regex con motor seguro frente a patrones patológicos (preferiblemente sin backtracking exponencial).
- Hashing y checksums: SHA-256 y algoritmos adicionales según necesidad; distinguir hashes de contraseñas de hashes rápidos.
- JSON: parseo, serialización y acceso seguro a campos.
- Pruebas de interoperabilidad y límites de tamaño para entradas externas.

### P4 — E/S y capacidades del sistema
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

### P6 — Módulos y errores del lenguaje
- Sistema de módulos con imports explícitos, resolución determinista y control de versiones.
- Separar biblioteca estándar y módulos opcionales.
- Diseñar manejo de errores recuperables: resultado explícito (`Result`/operador de propagación) como base; excepciones tipo `try/catch` solo si aportan una semántica clara.
- No ocultar errores ni convertir fallos en valores ambiguos.

### P7 — Tipado e inferencia
- Inferir tipos de parámetros a partir de usos y llamadas cuando sea seguro.
- Propagar tipos a través de ramas, retornos, colecciones, índices y funciones.
- Añadir anotaciones opcionales para parámetros/retornos y genéricos donde aporten valor.
- Mantener `unknown` como escape conservador cuando no haya evidencia suficiente; no inventar tipos.

### P8 — Rendimiento y compilación
- Crear benchmarks reproducibles: tiempo, memoria, profundidad máxima y coste de colecciones.
- Medir coste real de tokens con tokenizadores representativos y comparar legibilidad/tasa de generación correcta (Pass@1).
- Evaluar bytecode/IR y compilación solo después de tener benchmarks que demuestren el cuello de botella.
- Mantener CI con tests, lint, formato, fuzz smoke tests y benchmarks no bloqueantes.

## Criterios para aceptar cada capacidad

1. Pruebas unitarias y de integración, incluidos errores y límites.
2. Documentación con ejemplos ejecutables.
3. Sin regresiones en sintaxis legacy ni en modo compacto.
4. Medición cuando se alegue mejora de rendimiento o ahorro de tokens.
5. Límites seguros por defecto para operaciones de sistema y red.
