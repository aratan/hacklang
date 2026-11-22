# Modelo de memoria de HackLang

Estado: **propuesta normativa de arquitectura; todavía no implementada**.

Este documento define el contrato al que deberán ajustarse el intérprete Rust y el backend nativo. No cambia la semántica observable del lenguaje ni autoriza todavía a sustituir el runtime actual. La implementación se hará por fases y con pruebas diferenciales.

## 1. Objetivos y restricciones

HackLang adoptará un modelo **stack-first, sin GC y sin gestión manual por parte del usuario**:

- El compilador decide automáticamente dónde vive cada valor.
- Los valores locales de tamaño conocido viven preferentemente en la pila o en regiones de ámbito.
- Los datos que sobreviven a su ámbito se promocionan a almacenamiento de vida más larga.
- La liberación es determinista: ocurre al terminar el ámbito propietario o al destruir el propietario de una región promovida.
- No se exige al usuario escribir `&`, `&mut`, lifetimes, `move`, `malloc`, `free` ni llamadas de liberación.
- No se introduce un garbage collector general ni reference counting como mecanismo universal.
- El lenguaje conserva arrays, mapas, funciones de primera clase, closures mutables y recursividad del intérprete actual.

«Stack-first» es una preferencia de asignación, no la promesa de que todo quepa en la pila. Strings dinámicas, colecciones que crecen y valores que escapan necesitan almacenamiento de mayor duración.

## 2. Modelo de almacenamiento

### 2.1 Clases de almacenamiento

| Clase | Uso previsto | Recuperación |
|---|---|---|
| Valor inline | Number, Bool y estructuras pequeñas de tamaño conocido | Al salir del ámbito o destruir el contenedor |
| Stack frame | Variables locales y temporales no escapados | Al retornar o salir del ámbito |
| Scope region | Datos dinámicos temporales ligados a un bloque/iteración | Liberación de región al cerrar el ámbito |
| Owned region | Datos que sobreviven al frame creador: closures, arrays/maps/strings escapados | Al terminar el propietario que conserva la región |
| Static storage | Constantes y funciones sin capturas, cuando sea posible | Durante toda la ejecución |

Las regiones son conceptuales: la implementación puede usar arenas/segmentos y punteros de frontera, pero no debe filtrar detalles de asignación al lenguaje.

### 2.2 Data stacks y regiones

Inspiración: el enfoque de Goose de asignar y recuperar grupos de datos por ámbito, en lugar de liberar cada objeto individualmente.

- Cada ámbito dinámico tiene una región asociada.
- Las asignaciones temporales se agrupan; reservar espacio avanza un cursor.
- Al cerrar el ámbito, se destruyen los valores que requieren destructor y se recupera la región completa.
- Los bucles deben poder reutilizar la región de iteración sin acumular memoria entre iteraciones.
- Los datos que sobreviven a una región se promueven a una región propietaria de vida suficiente.
- La promoción debe preservar identidad y mutabilidad observables: no se permite copiar silenciosamente un objeto mutable si el programa puede observar la diferencia.

No se fija todavía una estrategia concreta de segmentos, tamaños iniciales o crecimiento. Eso depende de mediciones y de la implementación.

### 2.3 Escape analysis

El compilador realiza análisis de escape sobre el AST/representación de compilación:

- Un valor no escapa si todos sus usos terminan antes que su ámbito propietario.
- Escapa si se devuelve, se almacena en un objeto de vida más larga, se captura en una closure que sobrevive, o se pasa a una operación cuyo contrato permite conservarlo.
- El análisis es transitivo: si un objeto que escapa contiene referencias a otros valores, estos también deben sobrevivir.
- Si el análisis no puede demostrar seguridad, elige una región de vida más larga; nunca una referencia colgante.
- La decisión de ubicación no debe alterar el resultado del programa.

La primera versión puede ser conservadora. La optimización de stack no debe retrasar la corrección.

## 3. Propiedad y referencias inferidas

El contrato combina propiedad determinista inspirada en Rust con ubicación automática mediante escape analysis:

1. Todo valor tiene un propietario lógico.
2. Los usos temporales pueden tomar préstamos inferidos, sin sintaxis adicional.
3. Varios usos de solo lectura pueden coexistir.
4. Una mutación requiere exclusividad durante la operación; el compilador debe impedir aliasing mutable no seguro o introducir una representación interna segura.
5. Ninguna referencia puede sobrevivir al propietario del objeto referenciado.
6. Las asignaciones y retornos transfieren o prolongan la propiedad automáticamente según el análisis.
7. Los recursos con destructor se destruyen exactamente una vez.

En caso de duda, se promociona el almacenamiento; nunca se inventa una copia cuando la identidad observable pueda cambiar.

## 4. Contrato por tipo de HackLang

| Tipo actual | Representación preferida | Regla de vida |
|---|---|---|
| Number / Bool | Inline | Sin asignación dinámica |
| String | Inline si es pequeña y de tamaño fijo; región para contenido dinámico | Promoción si escapa |
| Array | Descriptor inline + buffer en región | Buffer ligado al propietario; crecimiento puede cambiar buffer sin invalidar referencias internas permitidas |
| Map | Descriptor inline + entradas en región | Igual que Array; conservar orden y semántica de claves |
| Function sin capturas | Código estático + descriptor pequeño | No necesita entorno dinámico |
| Closure | Código + entorno capturado | El entorno sobrevive exactamente tanto como la closure |
| AST y temporales del compilador | Propiedad interna de Rust | Fuera del contrato de memoria del programa ejecutado |

Los mapas actuales son pares ordenados y admiten las reglas de igualdad descritas en `NATIVE_COMPILER_DESIGN.md`; el modelo de memoria no puede cambiar su comportamiento.

## 5. Closures, recursividad y ciclos

Este es el punto de mayor riesgo porque el evaluador actual captura el entorno léxico por identidad y permite observar mutaciones posteriores.

Requisitos obligatorios:

- Una closure que escapa debe conservar todas las variables capturadas que necesite.
- Las capturas mutables deben seguir observando las modificaciones que hoy son visibles por semántica léxica.
- Una closure no puede guardar un puntero a un frame que ya se haya liberado.
- La recursividad y las referencias mutuas no pueden crear ciclos de propiedad que requieran GC o reference counting para liberar memoria.
- Las funciones sin capturas deben poder representarse sin entorno.
- La implementación debe separar referencias a código/funciones de la propiedad del entorno, y definir grupos de closures recursivas con un propietario común cuando sea necesario.
- No se puede resolver el problema copiando capturas mutables por valor: eso cambiaría el comportamiento actual.

Antes de implementar closures en C debe existir una representación probada de entorno capturado, propietario del entorno y grupo recursivo. Si no se puede demostrar liberación determinista, la característica no se habilita en el backend nativo.

## 6. Ámbitos y flujo de control

El modelo debe respetar los ámbitos existentes:

- `Statement::Block`, ramas de `if`, bucles `while` y llamadas crean ámbitos hijos.
- La condición de `if` se evalúa antes de crear el ámbito de la rama.
- El `while` conserva el ámbito del bucle entre iteraciones, pero los temporales exclusivos de una iteración se recuperan antes de la siguiente.
- `break`, `continue`, `return` y errores ejecutan los destructores y liberan las regiones abandonadas en orden inverso de creación.
- `Expression::Block` como cuerpo de función utiliza el frame de llamada actual, de acuerdo con la semántica ya documentada.
- La promoción de un valor devuelto debe ocurrir antes de destruir el frame que lo creó.

## 7. Errores, límites y seguridad

- La falta de memoria produce un error controlado; no debe convertirse en corrupción ni comportamiento indefinido.
- La aritmética de tamaños y capacidades debe comprobar overflow.
- Toda referencia interna debe tener propietario y región válidos.
- El runtime nativo debe liberar también en rutas de error y retornos tempranos.
- No se permiten punteros a stack almacenados en regiones de vida más larga.
- No se permiten referencias que sobrevivan a una iteración cuando apuntan a datos temporales de esa iteración.
- Los límites de profundidad actuales (256) siguen siendo independientes de la estrategia de memoria hasta que se diseñe y pruebe un cambio.

## 8. Diferencias entre runtime actual y arquitectura objetivo

| Área | Estado actual | Objetivo |
|---|---|---|
| Evaluador Rust | Arena de entornos con mark-and-sweep de frames; String/Vec/HashMap son gestionados por Rust | Propiedad/ámbitos deterministas y escape analysis; sin GC general |
| Runtime C | `malloc` para strings, entornos y bindings; no libera individualmente durante la ejecución | Regiones, promoción y destrucción determinista |
| Closures | ID estable de entorno capturado | Entorno con propietario explícito y ciclos resueltos estáticamente |
| Arrays / Maps C | Aún no implementados | Buffers ligados a regiones propietarias |
| Análisis de escape | No existe | Fase del compilador compartida por backend y futuras optimizaciones |

No describir el sistema actual como si ya implementara el modelo objetivo.

## 9. Secuencia de implementación

1. **Contrato y pruebas de diseño**: cerrar casos de ámbitos, escapes, mutabilidad, closures, recursividad, arrays/maps y errores. Sin cambios en runtime.
2. **Representación interna**: diseñar regiones, propietarios, descriptores y referencias; documentar invariantes y operaciones de promoción.
3. **Análisis conservador**: calcular escapes y emitir diagnósticos internos explicables. Sin optimizaciones agresivas.
4. **Evaluador Rust**: introducir el modelo como implementación de referencia y comparar resultados con el evaluador actual.
5. **Runtime C**: implementar regiones y liberación en todos los caminos; después arrays/maps/indexación.
6. **Closures y recursividad nativas**: solo cuando los ciclos y la propiedad tengan pruebas específicas.
7. **Optimización**: reducir promociones y asignaciones tras medir; añadir una futura orden `hl explain-memory` para explicar decisiones stack/region/owned.

No introducir una IR independiente solo para este trabajo. Reutilizar el AST y las fases actuales hasta que una necesidad real de análisis/optimización justifique una representación intermedia compartida.

## 10. Matriz de pruebas de diseño

La matriz contiene requisitos de conformidad. Se ha añadido una primera batería automatizada al evaluador actual para fijar semántica observable; esto **no** demuestra ubicación stack/region, liberación real ni paridad nativa del nuevo modelo. Los IDs implementados se enumeran después de la tabla; los demás siguen pendientes y no deben darse por cubiertos.

| ID | Caso | Resultado exigido |
|---|---|---|
| MEM-001 | Number/Bool local en función | Sin heap obligatorio; resultado idéntico |
| MEM-002 | String temporal no escapada | Región liberada al salir del ámbito |
| MEM-003 | Array local usado solo dentro del bloque | Buffer recuperado al cerrar el bloque |
| MEM-004 | Array devuelto por función | Buffer promovido; contenido válido tras el retorno |
| MEM-005 | Map insertado en otro objeto que escapa | Escape transitivo de entradas y valores |
| MEM-006 | String dentro de Array devuelto | String sigue válida; sin referencia colgante |
| MEM-007 | Closure devuelta que captura variable inmutable | Captura válida tras retornar la función creadora |
| MEM-008 | Closure devuelta que captura variable mutable | Mutaciones observables conservadas |
| MEM-009 | Dos closures comparten una captura mutable | Ambas observan la misma ubicación lógica |
| MEM-010 | Closure recursiva | Llamada correcta y liberación determinista |
| MEM-011 | Closures mutuamente recursivas | Grupo propietario sin ciclo no recuperable |
| MEM-012 | Closure almacenada en Array/Map que escapa | Entorno capturado sobrevive transitivamente |
| MEM-013 | Bucle de muchas iteraciones con temporales | Memoria temporal acotada; no crece por iteración |
| MEM-014 | `continue` y `break` desde bloques anidados | Regiones abandonadas se liberan en orden correcto |
| MEM-015 | `return` desde bloque/while | Valor promovido antes de destruir frames |
| MEM-016 | Error en expresión con objetos temporales | Destructores y regiones ejecutados sin fugas |
| MEM-017 | Reasignación de variable a nuevo Array/Map | Propietario anterior liberado cuando deja de ser necesario |
| MEM-018 | Referencia temporal usada después del ámbito | Rechazo estático o promoción; nunca dangling pointer |
| MEM-019 | Referencia mutable aliasada | Rechazo o serialización segura de la mutación |
| MEM-020 | Overflow de tamaño/capacidad | Error controlado |
| MEM-021 | Recursividad profunda | Límite controlado y limpieza de todos los frames |
| MEM-022 | Mismo programa interpretado y compilado | Salida, valores y errores equivalentes |

## 11. Cobertura automatizada actual

Pruebas añadidas en `src/evaluador.rs`:

| ID | Test | Qué fija hoy |
|---|---|---|
| MEM-004 | `mem_004_returned_array_remains_valid_after_creator_returns` | Array devuelto accesible tras retornar |
| MEM-005 | `mem_005_nested_map_and_array_values_survive_escape` | Acceso a mapa/array anidado tras retorno |
| MEM-006 | `mem_006_string_inside_returned_array_remains_valid` | String anidada en colección devuelta |
| MEM-007 | `mem_007_returned_closure_keeps_immutable_capture` | Captura inmutable en closure escapada |
| MEM-008 | `mem_008_returned_closure_observes_mutable_capture` | Mutación visible en llamadas posteriores |
| MEM-009 | `mem_009_sibling_closures_share_the_same_mutable_capture` | Dos closures comparten el mismo entorno lógico |
| MEM-010 | `mem_010_recursive_closure_remains_callable` | Recursividad de closure |
| MEM-011 | `mem_011_mutually_recursive_closures_resolve_shared_scope` | Resolución mutua dentro del entorno compartido |
| MEM-012 | `mem_012_closure_nested_in_escaping_collection_keeps_capture_alive` | Closure dentro de colección escapada |
| MEM-013 | `mem_013_repeated_loop_calls_reuse_environment_frames` | Reutilización de frames en llamadas repetidas |
| MEM-014 | `mem_014_break_and_continue_leave_nested_scopes_consistent` | Flujo de control en ámbitos anidados |
| MEM-015 | `mem_015_return_from_nested_control_flow_preserves_returned_value` | Retorno de colección desde un bucle |
| MEM-016 | `mem_016_error_during_nested_evaluation_is_reported` | Error de indexación propagado |
| MEM-017 | `mem_017_reassigning_collection_keeps_new_value_observable` | Semántica observable de reasignación |
| MEM-020 (parcial) | `mem_020_huge_collection_index_fails_without_panicking` | Índice enorme rechazado sin panic; no prueba overflow de capacidad |
| MEM-021 (parcial) | `mem_021_deep_recursion_fails_with_controlled_error` | Límite de profundidad; limpieza completa aún no instrumentada |
| MEM-022 (parcial) | `mem_022_collection_and_closure_semantics_are_stable_across_nested_calls` | Semántica del evaluador; no compara aún backend nativo |

Pendiente de instrumentación o de una futura fase del lenguaje:

- MEM-001..003: medir asignaciones y demostrar ubicación inline/stack/region.
- MEM-018..019: HackLang aún no expone referencias explícitas; definir y comprobar aliasing/vida cuando se diseñe esa capacidad interna.
- MEM-020 completo: probar overflow de tamaño/capacidad en el allocator de regiones.
- MEM-021 completo: instrumentar destrucción de frames/regiones en todos los retornos y errores.
- MEM-022 completo: comparar intérprete y binario para arrays, mapas y closures cuando el backend nativo los implemente.

Estas pruebas fijan el comportamiento de referencia; no se han sustituido todavía la arena mark-and-sweep del evaluador ni las asignaciones del runtime C.

## 12. Criterios de aceptación

No se declarará implementado el modelo hasta que:

- No exista GC general en el runtime de HackLang.
- No se exija gestión manual de memoria al usuario.
- Las referencias colgantes y el uso tras liberar sean imposibles por construcción o comprobación.
- Arrays, mapas, strings y closures tengan propietario y regla de destrucción definidos.
- Los ciclos de closures/recursividad se resuelvan sin GC ni refcount cíclico.
- Los retornos, errores y saltos de control liberen correctamente.
- La matriz de pruebas esté automatizada y el intérprete y el ejecutable nativo mantengan paridad.
- Se midan asignaciones y memoria máxima en programas representativos para verificar que stack-first aporta una mejora real.
