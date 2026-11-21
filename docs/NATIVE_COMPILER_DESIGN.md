# Diseño del intérprete y compilador nativo

Estado: especificación de diseño. No describe funcionalidades ya implementadas.

## Objetivo

HackLang debe ofrecer dos formas de ejecutar el mismo programa:

- Interpretación: el AST se evalúa con el runtime Rust existente.
- Compilación: el AST se transforma en código C, se enlaza con el runtime nativo de HackLang y un compilador C produce un ejecutable del sistema anfitrión.

Contrato de usuario previsto:

```sh
hl run programa.hl
hl build programa.hl
hl build programa.hl -o programa
hl check programa.hl
./programa
```

`hl programa.hl` seguirá siendo un alias de `hl run programa.hl`. Las opciones actuales `--compact`, `--format` y `--diagnostics-json` deben conservarse y no confundirse con los subcomandos.

## Estado verificado en el código actual

- Cargo define un único paquete y binario Rust `hl`; no hay dependencias externas.
- `main.rs` ya reutiliza lexer (`tokenize_spanned`), parser (`Parser::new_spanned` / `new_compact_spanned`), formatter, checker (`TypeChecker::check`) y evaluador (`Evaluator`). No hay subcomandos `run/build/check`; las opciones `--compact`, `--format` y `--diagnostics-json` sí existen y deben preservarse.
- El lexer ya produce `SpannedToken` y el parser acepta tokens con spans para enriquecer errores sintácticos. El AST (`Program`, `Statement`, `Expression`) todavía no conserva spans: no se debe volver a implementar tokenización ni localización desde cero. La mejora pendiente es propagar los spans existentes hasta nodos/diagnósticos del AST.
- El checker admite `Unknown` y es deliberadamente no bloqueante. No puede usarse como garantía de que todos los tipos están resueltos.
- `prototypes/output.c` es un prototipo histórico incompleto, no un backend reutilizable ni un runtime funcional.
- El evaluador limita la profundidad de evaluación a 256. El parser también tiene límites defensivos de anidamiento.
- La semántica que sigue se ha contrastado con `src/ast.rs` y `src/evaluador.rs`; debe convertirse en pruebas de conformidad antes de declarar paridad nativa.

## Semántica que el backend debe respetar

### Valores

El runtime actual tiene estos valores:

| Variante | Representación actual |
|---|---|
| Number | `f64` |
| Bool | booleano |
| String | cadena UTF-8 propiedad de Rust |
| Array | vector ordenado de Values |
| Map | vector ordenado de pares (Value, Value) |
| Function | parámetros, cuerpo Expression y entorno léxico capturado |

El runtime nativo utilizará valores etiquetados equivalentes. Los números deben conservar semántica IEEE-754 de doble precisión. Los strings deben conservar UTF-8 y las operaciones por índice/longitud deben contar caracteres Unicode, no bytes.

### Truthiness

Las condiciones de `if`, `while` y el ternario usan la misma conversión dinámica:

- Bool: su propio valor.
- Number: falso si es 0.0; verdadero en otro caso.
- String: falso si está vacía.
- Array, Map y Function: siempre verdaderos, incluso vacíos.

No introducir cortocircuito ni coerciones adicionales sin especificación y pruebas.

### Operadores

La implementación actual define para dos Numbers:

- `+`, `-`, `*`, `/`; división por cero devuelve error.
- `<`, `>` producen Bool.
- `~` compara igualdad numérica.
- `^` produce desigualdad numérica usando `abs(a-b) > f64::EPSILON`.

La suma Number/String y String/Number concatena la representación del número con la cadena. Otros operadores de esas combinaciones generan una cadena con formato `izquierda(derecha)`. Otras combinaciones de tipos producen error.

**Punto de saneamiento previo a paridad:** `num_op` devuelve actualmente Number(0) para tokens no contemplados y las operaciones String/Number generan texto para operadores distintos de `+`. El backend no debe inventar un comportamiento distinto silenciosamente. Primero hay que decidir si se conserva por compatibilidad o se corrige en el intérprete mediante un cambio documentado y tests; después ambos runtimes deben compartir el contrato corregido.

### Variables y ámbitos

- `let` crea o reemplaza una vinculación en el entorno actual.
- `name = value` actualiza la vinculación existente más cercana; falla si no existe.
- Los bloques como sentencia (`Statement::Block`), las ramas `if`, los bucles `while` y las llamadas crean entornos hijos.
- El cuerpo de un `while` comparte el entorno creado para ese bucle entre iteraciones.
- La condición de `if` se evalúa antes de crear el entorno de la rama.
- `Expression::Block` (bloque usado como cuerpo de lambda) ejecuta sus sentencias en el entorno de llamada ya creado; no crea otro entorno adicional.
- La asignación indexada solo acepta actualmente el nombre de una variable como destino directo.

### Funciones y flujo

- Las lambdas son funciones de primera clase y capturan el entorno léxico por identidad, no una copia de sus variables.
- Las closures observan mutaciones posteriores y admiten recursividad.
- La aridad es exacta; los argumentos se evalúan en el entorno llamador antes de entrar en el entorno capturado.
- Una llamada crea un entorno local cuyo padre es el entorno capturado.
- El cuerpo de función puede ser una expresión o un bloque; un bloque sin `return` produce Number(0).
- `return` propaga el valor a través de bloques, ramas y bucles.
- `break` y `continue` controlan el bucle más cercano; fuera de un bucle son errores.
- `return` en el nivel superior es error.
- La profundidad de evaluación está limitada actualmente a 256.

### Colecciones, indexación y builtins

- Los arrays son ordenados; el índice debe ser Number entero no negativo y estar dentro de rango.
- Los mapas son pares ordenados. La búsqueda compara Values con igualdad estructural derivada de `PartialEq` y devuelve la primera clave coincidente.
- Un literal de mapa puede contener claves repetidas; la lectura encuentra la primera. La asignación actualiza la primera coincidencia o añade una entrada al final.
- La igualdad de `Value` deriva de `PartialEq`; por tanto, incluye igualdad estructural de arrays/mapas/funciones y las reglas de `f64` de Rust. El backend debe fijar y probar esos casos, no asumir igualdad por identidad.
- La indexación de strings devuelve un string con el carácter Unicode en esa posición.
- `len(x)` acepta exactamente un argumento: String (número de caracteres Unicode), Array o Map. Otros tipos y aridades fallan.
- La asignación indexada de arrays exige que la posición ya exista; los mapas permiten insertar claves.

### Salida

- Number, Bool y String se imprimen con formato directo.
- Array y Map usan formato de depuración derivado de Rust, incluyendo las variantes de Value.
- Function imprime `<function>`.

La representación de colecciones es una interfaz observable y debe definirse explícitamente antes de la paridad: no se debe asumir que el formato de depuración de Rust coincide con el de C.

## Arquitectura objetivo

```text
                 ┌── Evaluator Rust ──┐
Source → Lexer → Parser → AST          ├→ comportamiento HackLang
                 └── C Code Generator → C source + native runtime → C compiler/linker → executable
```

### Inventario de reutilización: no duplicar

| Necesidad | Ya existe | Trabajo correcto |
|---|---|---|
| Tokenización y posiciones | `Lexer::tokenize_spanned`, `SpannedToken`, `SourceSpan` | Reutilizar; no crear otro lexer/span model |
| Parsing legacy/compacto | `Parser::new_spanned`, `new_compact_spanned` | Reutilizar el parser actual |
| AST | `Program`, `Statement`, `Expression` | Extender el AST solo para spans si se requiere; no crear un AST paralelo |
| Formateo | `formatter::format_program` | Mantenerlo independiente del compilador |
| Análisis estático | `TypeChecker::check`, `Unknown`, diagnósticos JSON | Reutilizarlo como análisis opcional, no como type lowering obligatorio |
| Ejecución de referencia | `Evaluator::evaluate` | Mantenerlo como motor de `run` y oráculo diferencial |
| Pruebas semánticas | Tests dentro de `evaluador.rs` y CI de ejemplos | Ampliar la suite existente y sumar harness diferencial, sin replicar tests manualmente |
| Backend C | Solo `prototypes/output.c`, incompleto | Crear backend/runtime nuevo y aislado; no presentar el prototipo como reutilizable |

## Decisiones iniciales

1. Mantener lexer, parser, AST y checker como front-end único.
2. Mantener el evaluador Rust como implementación de referencia y modo `run`.
3. Implementar inicialmente un generador C desde el AST, sin añadir una IR solo por anticipación. Revaluar una IR cuando optimizaciones, múltiples backends o complejidad real lo justifiquen.
4. El C generado utilizará un runtime dinámico con Values etiquetados. No dependerá de que el checker resuelva estáticamente cada tipo.
5. El runtime nativo debe gestionar entornos, closures, strings, arrays, mapas, errores y memoria. No usar refcount simple sin resolver ciclos de closures recursivas.
6. El compilador C se invocará como proceso externo. Primera plataforma: Linux x86-64 con GCC o Clang disponible. Detectar el compilador, comprobar su versión/ejecución y devolver errores legibles. No afirmar que el binario generado es independiente de libc o de la plataforma.
7. La salida por defecto será el nombre del fuente sin extensión; `-o` permite elegir destino. No sobrescribir entradas ni archivos existentes sin una política explícita.
8. Los errores actuales del CLI no establecen códigos de salida no-cero de forma consistente; corregir esto en la refactorización del CLI antes de usarlo en automatización.
9. No duplicar funciones de parseo, lectura de fuente, chequeo, formato o ejecución para cada subcomando: extraer un pipeline compartido y enrutar las acciones después del parseo.
10. Los temporales se crearán en un directorio controlado y se limpiarán tanto en éxito como en error. Los errores de compilación deben conservar diagnósticos útiles.

### Runtime nativo

Separar, al menos conceptualmente, estos módulos:

- `Value`: tag + payload para Number, Bool, String, Array, Map y Function.
- `Environment`: tabla de bindings y enlace al padre.
- `Function/Closure`: parámetros, cuerpo generado y entorno capturado.
- `String`: UTF-8 y operaciones por carácter Unicode.
- `Array/Map`: orden y reglas de igualdad/indexación compatibles.
- `Control flow`: retorno, break, continue y propagación de errores.
- `Memory`: política explícita para objetos y entornos, con ciclos de closures cubiertos.
- `Diagnostics`: errores de runtime consistentes con el intérprete.

No implementar filesystem, procesos, sockets ni biblioteca estándar de sistema como parte de esta primera fase: son capacidades distintas y siguen pendientes en el roadmap.

## Plan incremental y criterios de aceptación

### N0 — Contrato semántico y conformidad

- Reutilizar la batería existente de tests de `evaluador.rs` (ya cubre closures, recursividad, scopes, GC de frames, truthiness, operadores básicos, colecciones, `len` y límites); ampliarla donde haya huecos, no duplicarla en otra suite Rust paralela.
- Corregir el fallback silencioso de operadores no soportados en `num_op` antes de definir el contrato nativo.
- Convertir los casos existentes del evaluador en corpus de conformidad.
- Añadir tests para todos los operadores, coerciones, mapas con claves repetidas, impresión, Unicode, scopes, closures, errores y límites.
- Resolver los comportamientos ambiguos señalados en operadores y formato de colecciones.
- Cada test debe describir resultado esperado o error esperado.

Aceptación: contrato escrito y tests Rust que lo fijan.

### N1 — CLI y toolchain

- Reutilizar el pipeline actual de lectura → lexer spanned → parser → formatter/checker/evaluator. Extraerlo a funciones compartidas si hace falta para que `run`, `check` y `build` no implementen front-ends separados.
- Introducir parser de argumentos para `run`, `build` y `check`, manteniendo aliases y flags actuales.
- Corregir códigos de salida: ahora `main` imprime errores y hace `return`, lo que termina con código de proceso 0. Errores de argumentos, lectura, lexer, parser, runtime y toolchain deben devolver códigos distintos de cero.
- Comprobar disponibilidad de GCC/Clang solo para `build`.
- Definir códigos de salida y mensajes de error estables.

Aceptación: pruebas CLI de uso, argumentos inválidos, archivo inexistente y ausencia de toolchain.

### N2 — Runtime C mínimo

- Implementar Value, errores, print, entornos y operaciones primitivas.
- Compilar y ejecutar un programa C de smoke test desde CI Linux.

Aceptación: tests nativos del runtime sin depender todavía de todo el lenguaje.

### N3 — Backend por subconjuntos

Orden sugerido:
1. literales, variables, print y aritmética;
2. comparaciones, truthiness, if y ternario;
3. while, break y continue;
4. arrays, mapas e indexación;
5. funciones, closures, recursividad y return;
6. builtins y errores restantes.

Cada ampliación debe añadir pruebas diferenciales contra el intérprete.

### N4 — Build y pruebas diferenciales

- `hl build` genera C temporal, compila y enlaza runtime y produce un ejecutable nativo.
- CI ejecuta cada programa compatible con ambos motores y compara stdout, stderr y código de salida.
- Los errores se comparan por categoría/código estable, no necesariamente por texto idéntico mientras los mensajes no estén normalizados.
- Añadir pruebas de memoria y ejecución repetida para detectar fugas/corrupción.

Aceptación: ningún programa dentro del subconjunto declarado produce divergencias sin documentar.

### N5 — Cobertura y portabilidad

- Ampliar hasta cubrir todo el AST y la semántica del intérprete.
- Solo entonces declarar paridad completa.
- Añadir ARM64/Windows como objetivos separados con toolchains y CI reales.

## No objetivos de la primera entrega

- LLVM, Cranelift, bytecode o optimizaciones complejas antes de medir necesidad.
- Cross-compilation y múltiples arquitecturas.
- Sistema de módulos, paquetes, filesystem, red, procesos o concurrencia.
- Hacer obligatorio el análisis estático para compilar.
- Reutilizar `prototypes/output.c` como si fuera un backend operativo.

## Regla de calidad

Nunca anunciar «compilación nativa completa» mientras exista una variante AST, un comportamiento observable o un caso de error del intérprete sin soporte nativo y sin una exclusión explícita del subconjunto soportado.
