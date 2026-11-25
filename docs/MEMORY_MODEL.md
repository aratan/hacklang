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

### 2.4 Sintaxis de ámbitos: implícita, no anotada

HackLang **no añadirá una sintaxis especial de memoria o de región** en la primera versión del modelo. Los ámbitos léxicos que ya reconoce el parser son la fuente única de delimitación:

- Modo legacy: bloques delimitados por `(...)`.
- Modo compacto: bloques delimitados por `{...}`.
- Las ramas, cuerpos de bucle y llamadas conservan las reglas de ámbito ya definidas en la gramática y en el evaluador.

El compilador deriva de esos nodos del AST los límites de vida y las regiones temporales. Al cerrar un ámbito, destruye sus valores locales y recupera su región, salvo los valores que hayan escapado. Estos últimos se promocionan automáticamente a una región propietaria de vida suficiente; el código fuente no cambia.

Ejemplo legacy:

```hacklang
let message = "hello";
if (ready) (
  let temp = build(message);
  print temp;
)
```

Ejemplo compacto:

```hacklang
message := "hello"
if ready {
  temp := build(message)
  print temp
}
```

En ambos casos el bloque determina el ámbito. No se escribe `scope`, `region`, `free` ni una anotación de lifetime. Si `temp` no escapa, sus datos temporales se recuperan al salir del bloque. Si el resultado se devuelve, se captura en una closure que sobrevive o se almacena en un objeto de vida mayor, el análisis de escape prolonga automáticamente su almacenamiento.

**Regla de diseño:** la sintaxis expresa intención y estructura del programa; el compilador decide la estrategia de memoria. No se añadirá sintaxis de propiedad (`move`), préstamo (`&`, `&mut`) o lifetime al código ordinario. Una anotación explícita solo se reconsiderará si aparece un caso real que no pueda resolverse de forma segura e inequívoca por inferencia, y deberá ser opcional.

No debe confundirse un bloque léxico con una garantía física de stack: el bloque fija la vida lógica; el análisis de escape decide si el almacenamiento puede ser stack/region local o debe promocionarse. Esta distinción evita referencias colgantes sin obligar a la IA a generar anotaciones redundantes.

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

### 4.1 Semántica de valor de arrays y mapas (hallazgo verificado)

El evaluador actual implementa colecciones con semántica de **valor**, no identidad compartida:

- Leer un identificador clona el `Value`; para Array y Map, el `Clone` de Rust copia recursivamente sus elementos y entradas.
- Leer un elemento por índice también devuelve un valor clonado.
- Por tanto, `b = a` seguido de una mutación de `b` no modifica `a`.
- Las closures son la excepción relevante: clonar un valor Function conserva el mismo `captured_env`, de modo que las variables capturadas siguen compartidas por identidad.

Consecuencias para el compilador:

- La promoción de arrays/mapas puede copiar sus buffers y valores de forma recursiva sin romper identidad observable, porque esa identidad compartida no existe en la semántica actual.
- La copia debe incluir strings y colecciones anidadas; no basta con copiar el descriptor.
- Las closures contenidas en una colección no se duplican semánticamente: sus handles siguen apuntando al mismo entorno capturado, que debe sobrevivir al escape de la colección.
- Una futura optimización copy-on-write solo será válida si conserva exactamente esta semántica y no añade aliasing observable.
- No introducir referencias mutables compartidas a colecciones sin una modificación explícita de la semántica del lenguaje.

Esta regla se fija con las pruebas MEM-023 y MEM-024. Es un dato importante para simplificar la promoción de datos; no resuelve por sí solo los ciclos de entornos creados por closures recursivas.

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

## 7.1 Primer planificador HIR (implementación inicial)

Se ha añadido `src/memory.rs` como primera fase ejecutable del análisis de almacenamiento. Se invoca durante `hl build` inmediatamente después de AST→HIR y antes de generar C.

El planificador actual es deliberadamente conservador:

- Number y Bool se clasifican como `Inline`.
- String literal o concatenación se clasifica inicialmente en `ScopeRegion` del binding.
- Una asignación posterior con string o valor de origen desconocido promueve el binding a `OwnedRegion`.
- Las expresiones leídas desde otro binding se consideran desconocidas; ante la duda se promociona.
- Las expresiones ternarias combinan conservadoramente las necesidades de ambas ramas.

Incluye pruebas unitarias para escalares inline, strings locales, concatenación y promoción tras asignación.

**Estado actual del planificador y runtime:** `MemoryPlan` clasifica bindings, calcula CFG/liveness, interferencias, slots, lanes por profundidad y operaciones de promoción. `hir_native.rs` consume `scope_stacks` y crea lanes físicas en el runtime C; los ámbitos hermanos pueden reutilizar una lane y los anidados usan lanes distintas. Los bindings `Inline` del módulo se materializan en slots físicos de `HlFrame`. Los valores dinámicos usan arenas bump regionales y, cuando necesitan sobrevivir al ámbito de origen, almacenamiento propiedad del binding y promoción por copia. El runtime clona recursivamente arrays y mapas para preservar su semántica de valor. La limpieza se ejecuta al cerrar entornos y en las rutas de `break`, `continue` y error fatal cubiertas por pruebas. Esto es una integración operativa para el subconjunto actual, no una escape analysis transitiva completa ni un sistema general de movimiento/propiedad para todos los grafos y closures.

**Slots planificados en el C generado:** el generador emite ahora, junto a la clase de almacenamiento de cada binding, su `DataStackId` y slot cuando existe una asignación en `MemoryPlan.binding_slots`. También rechaza asignaciones de slots para bindings inexistentes. Las pruebas verifican la emisión y el rechazo de metadatos huérfanos. Sigue siendo información de compilación: los slots aún no determinan direcciones ni asignaciones físicas del runtime.

**Validación previa del plan:** antes de generar C, `validate_slot_plan` comprueba que cada binding interferente tenga slot, que la pila coincida con su clase de almacenamiento, que bindings interferentes no compartan slot en la misma pila y que los conteos declarados coincidan con las asignaciones. Esta validación detecta inconsistencias del planificador; no constituye todavía una asignación física en el runtime.

**Conexión del planificador con generación C (integración parcial):** `compile_to_executable` pasa `MemoryPlan` a `hir_native::generate_c`. El generador valida las clasificaciones y consume `scope_stacks` para crear lanes físicas; los bindings `Inline` del módulo se asignan a slots reales de `HlFrame`. Los valores dinámicos usan las políticas de ámbito/propietario del runtime. No todos los slots regionales se materializan como slots individuales, ni existe todavía un frame ABI para activaciones de funciones.

### 7.2 Intervalos de uso conservadores (implementación inicial)

Cada binding HIR registra ahora posiciones de declaración, primer uso, último uso y última escritura. El recorrido asigna índices de evento monotónicos; los accesos dentro de bucles prolongan conservadoramente la vida hasta el límite de salida del bucle y se marcan como `loop_carried`. Los recorridos de ramas se serializan en el orden del HIR, lo que puede alargar intervalos, pero no acortarlos deliberadamente.

Los intervalos de uso siguen siendo metadata conservadora y no constituyen por sí solos una prueba de no-aliasing ni de escape. El planificador ya construye además un CFG explícito, resuelve liveness hasta punto fijo y deriva un grafo de interferencia y asignaciones de slots. Esas decisiones se validan antes de codegen; las lanes regionales y los slots inline del módulo sí se consumen físicamente. El análisis todavía no modela escapes transitivos generales ni todos los efectos de futuras llamadas nativas.

### 7.3 CFG y análisis de variables vivas (primera implementación)

El planificador construye ahora un grafo de flujo de control a partir del HIR estructurado. Los bloques representan declaraciones, asignaciones, lecturas/impresiones, condiciones, uniones de ramas y cabeceras/salidas de bucles. Se registran sucesores explícitos, incluidos los retrocesos de `while` y las salidas de `break` y `continue`.

Sobre ese grafo se resuelve el análisis clásico de variables vivas hasta punto fijo:

```text
live_out[B] = unión(live_in[S]) para cada sucesor S
live_in[B]  = use[B] ∪ (live_out[B] - def[B])
```

El resultado se expone en `MemoryPlan.cfg`. Se han añadido pruebas de propagación por ramas/uniones y de aristas de retroceso de bucles.

**Límite actual:** el CFG modela `return`, `break` y `continue` en el HIR estructurado del módulo, además de ramas y retrocesos de bucles. No construye todavía CFG independientes para cada activación de función ni modela efectos de llamadas directas ya habilitadas, destrucción de todos los temporales tipados o escapes transitivos. La asignación de slots se valida; el backend materializa slots inline y lanes regionales, pero no todos los slots planificados corresponden a ubicaciones físicas independientes.

### 7.4 Interferencia y asignación de slots (metadata)

A partir de `live_in` y `live_out`, el planificador construye un grafo de interferencia. Dos bindings se conectan cuando el análisis los considera simultáneamente vivos en un bloque, o cuando una definición coincide con un valor que sigue vivo a la salida del bloque. La asignación usa un coloreado voraz determinista: cada color representa un slot dentro de una pila de datos.

La reutilización está particionada por pila:
- Los valores `Inline` usan un espacio de slots de frame.
- Los valores `ScopeRegion(scope)` usan la lane asignada al ámbito.
- Los valores `OwnedRegion` quedan fuera del coloreado y no comparten slots por esta fase.

El resultado se expone como `MemoryPlan.interference`, `binding_slots` y `stack_slot_counts`. Hay pruebas para vidas no solapadas, bindings simultáneamente vivos y temporales de ramas mutuamente excluyentes.

La asignación de slots sigue siendo metadata para las lanes regionales y los valores dinámicos, pero los slots `Inline` del módulo ya se materializan en un `HlFrame` C y las `ScopeStackPlan` se convierten en lanes bump físicas. Los valores de vida desconocida quedan conservadoramente fuera de la reutilización corta y se asignan a `OwnedRegion`.

### 7.5 Simulador seguro de regiones bump (Rust)

Se añade `src/region.rs` como prototipo ejecutable y aislado del runtime productivo:

- Reserva bump de rangos de bytes con alineación potencia de dos y comprobación de overflow.
- Devuelve offsets (`Range<usize>`), nunca punteros crudos.
- Los bytes recién reservados quedan inicializados a cero.
- `mark()` captura cursor y número de recursos registrados; `restore()` retrocede el cursor y destruye los recursos añadidos desde la marca, en orden LIFO.
- `track<T>()` permite probar destructores deterministas.
- La capacidad física se conserva tras `restore` para reutilizarla; `used()` informa del tamaño lógico ocupado.

Se registran pruebas de alineación, inicialización, restauración anidada, orden de destrucción y accesos fuera de rango. El módulo se compila dentro del binario, pero **no está conectado al planificador HIR, al ABI ni al runtime C**. Es un simulador de offsets y ciclo de vida, no una arena de objetos HackLang ni una garantía de seguridad de referencias. La integración requerirá descriptores tipados, propiedad explícita de los valores y pruebas diferenciales antes de sustituir cualquier asignación actual.

### 7.6 Descriptores y tabla de regiones

`src/region.rs` incorpora ahora `RegionId`, `RegionDescriptor`, `RegionState`, `RegionCheckpoint` y `RegionTable`.

- Cada descriptor tiene identificador estable, padre opcional y estado explícito `Active/Closed`.
- La tabla valida que el padre exista y siga activo al crear una región hija.
- Las reservas y checkpoints se realizan a través del identificador de región.
- Un padre no puede cerrarse mientras tenga hijos directos activos; la regla se aplica transitivamente al cerrar cada nivel.
- Cerrar una región restaura su marca inicial, ejecuta los destructores registrados y bloquea operaciones posteriores.
- Los checkpoints pertenecen a una región concreta; no se pueden aplicar a otra región ni después de cerrarla.

También se corrige la reutilización de memoria: cada rango reservado se pone a cero incluso cuando reutiliza capacidad existente después de un `restore`. Así no se exponen bytes residuales de una asignación anterior.

**Límite:** la tabla es todavía un modelo Rust de propiedad y ciclo de vida. No contiene referencias entre objetos HackLang, no realiza promoción entre regiones y no cambia el ABI ni el runtime C. El siguiente paso será definir y probar la promoción de valores entre regiones antes de conectar los descriptores al planificador HIR.

### 7.7 Promoción transitiva de objetos (prototipo Rust)

La tabla de regiones incorpora un registro de objetos lógicos con handles estables (`RegionObjectId`), payload y referencias a otros objetos. Los handles son índices; no son punteros a memoria móvil.

`RegionTable::promote(root, destination)`:
- exige que el destino esté activo y sea la misma región propietaria o un ancestro del propietario del objeto raíz;
- recorre transitivamente el grafo y copia payloads;
- conserva referencias compartidas y ciclos mediante una tabla de correspondencias;
- asigna todos los objetos promovidos al destino;
- mantiene intacto el grafo original hasta que se cierre su región;
- al cerrar una región, invalida sus handles sin reciclar identificadores.

La creación de objetos impide que un objeto de vida larga retenga una referencia a un objeto de una región descendiente de vida más corta. Así, cerrar una región hija no deja referencias colgantes desde objetos del padre.

Se añaden pruebas para promoción transitiva, ciclos, destinos inválidos, invalidación al cerrar y rechazo de referencias con vida incompatible.

**Límite importante:** el registro es un modelo de objetos independiente de los bytes reservados por `Region::alloc_bytes`. La promoción copia `Vec<u8>` y la lista de handles en Rust; no mueve ni reescribe objetos del runtime C, no conecta todavía el HIR y no resuelve los grupos de closures capturadas. Es una prueba del contrato de vida/propiedad, no la implementación final de promoción sin copias.

## 8. Diferencias entre runtime actual y arquitectura objetivo

| Área | Estado actual | Objetivo |
|---|---|---|
| Evaluador Rust | Arena de entornos con mark-and-sweep de frames; String/Vec/HashMap son gestionados por Rust | Propiedad/ámbitos deterministas y escape analysis; sin GC general |
| Runtime C | Arenas bump por lanes/ámbitos, slots físicos `HlFrame` para bindings inline, buffers reutilizables propiedad de bindings, copia profunda de arrays/maps y promoción por copia en los casos conectados; limpieza LIFO en cierres y errores fatales probados | Ampliar propiedad/escape de todos los tipos y temporales, y habilitar movimiento solo con pruebas de vida y aliasing |\n| Planificador HIR | `src/memory.rs` calcula CFG/liveness, interferencias, slots, lanes, tokens y promociones; el backend consume lanes y slots inline y valida el plan | Escape analysis transitiva/path-sensitive y planificación de todos los temporales/activaciones |
| Closures | HIR, análisis de capturas, celdas compartidas y descriptores runtime; llamadas directas habilitadas con frames de activación, ABI de argumentos/closure, instalación de celdas de captura y retorno saneado; callees dinámicos y recursividad rechazados en compilación; celdas retenidas conservadoramente hasta teardown raíz | Raíces temporales, callees dinámicos y recolección segura de ciclos |
| Arrays / Maps C | Implementados en runtime; clonación recursiva, indexación y asignación anidada; paridad diferencial de arrays presente, cobertura de mapas por ampliar | Completar paridad, errores y garantías de vida/propiedad en todos los caminos |
| Análisis de escape | No existe | Fase del compilador compartida por backend y futuras optimizaciones |

No describir el sistema actual como si ya implementara el modelo objetivo.

## 9. Secuencia de implementación

1. **Contrato y pruebas de diseño — en curso:** mantener las pruebas semánticas del intérprete y ampliar paridad nativa, especialmente mapas, errores y casos de vida útil.
2. **Representación interna — parcial:** HIR, `MemoryPlan`, lanes, slots, tokens y operaciones de promoción ya existen; los modelos `RegionTable`/`OwnershipLedger` siguen siendo prototipos Rust y no deben confundirse con el runtime productivo.
3. **Análisis conservador — parcial:** CFG/liveness, interferencias y decisiones de almacenamiento están conectados; falta escape analysis transitiva y sensible a caminos para todos los tipos/temporales.
4. **Runtime C — integrado para el subconjunto actual:** arenas bump, lanes físicas, slots inline, almacenamiento owned y limpieza de ámbitos/salidas probadas. No hay todavía propiedad general de grafos ni GC de ciclos de closures.
5. **Funciones y closures invocables — parcial:** ABI con closure, cuerpos C, frames de activación, celdas de captura e identidad compartida implementados para llamadas directas, con pruebas diferenciales y sanitizers; faltan callees dinámicos (higher-order) y la vida de ciclos antes de habilitarlos.
6. **Recursividad nativa**: solo cuando las celdas raíz tengan trazado de ciclos, raíces temporales y pruebas específicas. Hoy el compilador rechaza cualquier ciclo de llamadas.
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
| MEM-023 | Copia de Array y mutación posterior | La mutación de la copia no altera el original |
| MEM-024 | Copia de Map con colección anidada | La mutación de la copia no altera el original ni sus valores anidados |

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
| MEM-023 | `mem_023_array_assignment_has_value_semantics_not_shared_identity` | Mutar una copia de array no modifica el original |
| MEM-024 | `mem_024_map_assignment_copies_nested_collection_values` | Mutar una copia de mapa no modifica el original anidado |

| MEM-025 | `unknown_read_assignment_is_promoted_instead_of_assumed_stack_safe` | Lectura de vida desconocida promueve a región propietaria |
| MEM-026 | `conditional_string_assignment_is_promoted_across_all_control_flow_paths` | Asignación dinámica en cualquier rama promueve el binding |
| MEM-027 | `string_temporary_from_outer_binding_is_never_confined_to_inner_scope` | Un valor leído desde fuera no se confina a una región interior |
| MEM-020 (parcial) | `mem_020_huge_collection_index_fails_without_panicking` | Índice enorme rechazado sin panic; no prueba overflow de capacidad |
| MEM-021 (parcial) | `mem_021_deep_recursion_fails_with_controlled_error` | Límite de profundidad; limpieza completa aún no instrumentada |
| MEM-022 (parcial) | `mem_022_collection_and_closure_semantics_are_stable_across_nested_calls` | Semántica del evaluador; no compara aún backend nativo |

Pendiente de instrumentación o de una futura fase del lenguaje:

- MEM-001..003: medir asignaciones y demostrar ubicación inline/stack/region.
- MEM-018..019: HackLang aún no expone referencias explícitas; definir y comprobar aliasing/vida cuando se diseñe esa capacidad interna.
- MEM-020 completo: probar overflow de tamaño/capacidad en el allocator de regiones.
- MEM-021 completo: instrumentar destrucción de frames/regiones en todos los retornos y errores.
- MEM-022 completo: comparar intérprete y binario para arrays, mapas y closures cuando el backend nativo los implemente.

MEM-023 y MEM-024 fijan además la semántica de valor de las colecciones. Estas pruebas fijan el comportamiento de referencia; no se han sustituido todavía la arena mark-and-sweep del evaluador ni las asignaciones del runtime C.

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

### 2.5 Planificación de pilas de datos (arquitectura propuesta)

El modelo evolucionará desde «una región por ámbito» hacia un plan de **N pilas de datos por activación**, asignadas por el compilador. Un ámbito lógico no implica necesariamente una reserva física propia. Varios ámbitos con vidas no solapadas podrán reutilizar la misma pila.

Cada pila de datos tendrá un descriptor interno:

| Campo | Función |
|---|---|
| `owner` | Frame, grupo de closures o propietario de mayor duración que controla la pila |
| `base` / `cursor` | Inicio y frontera actual de asignación |
| `capacity` | Capacidad del segmento activo |
| `parent` | Propietario al que se transfiere/promueve el contenido si escapa |
| `generation` | Detección interna de handles usados después de liberar una región |
| `lane` | Clase de vida asignada por el planificador |

El descriptor es un concepto de diseño, no una estructura implementada. Los punteros y generaciones no serán visibles en HackLang.

#### 2.6.1 Decisiones explícitas de promoción (primera implementación)

El planificador ahora emite `MemoryOperation::Promote` para asignaciones dinámicas cuyo destino tiene una vida léxica mayor que el ámbito donde se produce el valor. `MemoryOperation` es una representación neutral respecto al backend: Rust y C deben consumir la misma decisión y no volver a ejecutar análisis de escape independientes. En esta fase solo existe la operación `Promote`; las operaciones de reserva, movimiento y destrucción se incorporarán cuando sus contratos estén definidos. Cada decisión distingue explícitamente tres dimensiones: `ValueOrigin` (literal, lectura de binding, expresión calculada o desconocido), `StorageOwner` (propietario lógico actual: inline, ámbito, binding propietario o desconocido) y `destination_owner` (binding que recibirá la vida extendida). Una lectura de binding **no** se interpreta como movimiento de propiedad: el origen y el propietario se registran por separado. `StorageOwner::Binding` es metadata conservadora, no un handle físico ni prueba de que el runtime ya controle esa región. Cada decisión registra además el ámbito de producción, el ámbito destino y el motivo:

- `AssignedToLongerLivedBinding`: un valor dinámico producido dentro de un ámbito se asigna a un binding de un ámbito ancestro.
- `ReadFromShorterLivedBinding`: un valor leído desde un binding de un ámbito descendiente se asigna a un binding de vida mayor.

El análisis evita emitir promoción cuando el valor leído ya pertenece a un ámbito que vive al menos tanto como el destino. Se añaden pruebas para ambos escapes, asignaciones dentro del mismo ámbito y lecturas desde un ámbito exterior. La clasificación de expresiones también propaga hechos conocidos de bindings a través de `Read`, concatenaciones `+` y ramas ternarias: una lectura de un binding `Inline` se considera escalar; una lectura de un binding con almacenamiento dinámico conserva esa condición dentro de la expresión. Los bindings con origen directo en otro binding dinámico siguen en `OwnedRegion`, porque esta fase aún no representa alias/propietario compartido ni realiza transferencia física. La propagación es conservadora y depende de las clases inferidas en el recorrido HIR; no es todavía un análisis SSA sensible a caminos ni un sistema completo de tipos.

**Límite:** `MemoryOperation::Promote` se valida y el codegen la conecta con el runtime C para las asignaciones cubiertas. La promoción actual es por copia, no movimiento ni transferencia general de grafos. El análisis no es todavía transitivo ni path-sensitive para todas las combinaciones de arrays, mapas, closures, retornos y llamadas; las llamadas nativas siguen deshabilitadas. El evaluador Rust no consume estas operaciones como un allocator alternativo.


### 2.6.2 Modelo verificable del ciclo de vida del propietario

Se añade en `memory.rs` un modelo neutral de estado para una asignación dinámica:

- `OwnershipTokenId` identifica una asignación lógica; no es un puntero ni un handle del runtime.
- `Acquire` crea un único token con propietario inicial.
- `Move` cambia el propietario lógico del mismo token; no duplica el recurso.
- `Promote` cambia el propietario a uno de vida mayor, conservando la identidad del token.
- `Drop` marca el token como destruido y conserva una lápida para rechazar un segundo `Drop`.

El `OwnershipLedger` comprueba propietario esperado, rechaza propietarios desconocidos/inline para recursos dinámicos, detecta tokens inexistentes y evita una segunda destrucción. Los errores no mutan el registro. La relación «más larga» de `Promote` sigue siendo una precondición que debe demostrar el análisis llamador: el ledger no puede inferir jerarquía de vidas a partir de `StorageOwner` por sí solo. Las pruebas cubren movimiento, promoción, propietario incorrecto, token desconocido y doble destrucción. El ledger mantiene además una pila de registros activos por propietario: `acquire` apila, `Move`/`Promote` transfieren el mismo registro al destino, `drop_token` lo desactiva y `drain_owner` consume los registros en orden LIFO devolviendo la secuencia explícita de acciones `Drop`. Esto permite verificar el orden y la transferencia sin confundir el registro lógico con callbacks físicos del runtime. Las pruebas cubren también drenaje LIFO, transferencia de registros entre propietarios y retirada tras un `Drop` explícito. Una prueba adicional fija el orden esperado cuando se mueve un registro antiguo a otro propietario: el propietario origen drena sus registros restantes y el destino ejecuta después el registro transferido. El runtime C tiene una prueba independiente del mismo principio de inserción LIFO. Esto establece una expectativa de conformidad, pero todavía no es una prueba diferencial que ejecute ambos backends a partir de una misma secuencia de `OwnershipAction`.

**Alcance exacto:** esto es un modelo ejecutable de contrato y una base de pruebas, no un registro de destructores conectado a las asignaciones reales. No demuestra todavía orden LIFO, limpieza ante errores/retornos, ni liberación física exactamente una vez. Tampoco convierte automáticamente una lectura HIR en `Move`: el análisis actual no prueba exclusividad ni ausencia de alias. La integración con runtime C y evaluador Rust queda bloqueada hasta que ambos puedan asociar cada recurso real a un token y consumir las mismas acciones.

### 2.6 Hechos de uso disponibles en HIR (implementación inicial)

El planificador recoge ahora, por cada binding:

- Número de lecturas detectadas en expresiones HIR.
- Número de asignaciones posteriores a la declaración.
- Conjunto ordenado de ámbitos léxicos desde los que se lee.

Estos datos permiten construir y depurar el futuro análisis de vida y escape. **No constituyen todavía un análisis de liveness**: no modelan orden de ejecución, caminos alternativos, iteraciones ni puntos de salida. Por tanto, no autorizan a acortar la vida de un valor, reutilizar una lane adicional ni liberar almacenamiento. El plan actual sigue siendo conservador y el backend/runtime no consumen esta metadata.

### 2.5.1 Clases de vida y asignación

El análisis asignará cada valor dinámico a una de estas clases:

1. **Inline/frame:** valor pequeño y de tamaño conocido, almacenado directamente en el frame o dentro de su contenedor.
2. **Data stack local:** temporal dinámico cuya vida está contenida en una activación o conjunto de ámbitos.
3. **Data stack de ámbito:** datos que deben sobrevivir a varios bloques internos, pero no a su propietario.
4. **Owned data stack:** datos que escapan del frame creador y pasan a un propietario de vida mayor (por ejemplo, el llamador o un grupo de closures).
5. **Static:** constantes y código sin estado capturado.

Owned data stack no significa «heap libre sin dueño»: es una región con propietario explícito y destrucción determinista. La reserva física podrá proceder de segmentos obtenidos al crecer la región, pero toda reserva deberá pertenecer a un propietario recuperable.

#### 2.5.2 Asignación de lanes

El compilador construirá primero intervalos de vida para los valores dinámicos. Dos valores solo podrán compartir lane si sus vidas no se solapan o si sus asignaciones son estrictamente anidadas y el orden de restauración lo permite.

La asignación inicial será conservadora:

- Las vidas anidadas podrán compartir una lane mediante cursor y marcas (*mark/restore*).
- Las ramas mutuamente excluyentes podrán reutilizar espacio si el análisis demuestra que no hay salida de una rama que conserve referencias a la otra.
- Las iteraciones podrán restaurar la marca de temporales al final de cada vuelta; los valores que sobreviven a la iteración se asignarán a una lane exterior.
- Las llamadas recursivas tendrán descriptores por activación; nunca compartirán un cursor mutable global.
- Si no se demuestra que dos vidas son compatibles, se asignarán a lanes distintas o a un propietario de vida mayor.

La primera versión no intentará minimizar globalmente el número de lanes. La prioridad será que el plan sea determinista, verificable y correcto. La reducción de lanes será una optimización posterior basada en mediciones.

#### 2.5.3 Bump allocation, marcas y restauración

Una asignación en una lane lineal avanza su cursor alineado. Al entrar en un ámbito o comenzar una iteración se guarda una marca; al abandonarlo se ejecutan primero los destructores registrados y después se restaura el cursor a esa marca.

El orden de salida será siempre inverso al de adquisición. La misma secuencia de limpieza se generará para salida normal, `break`, `continue`, `return` y propagación de error. No se permite restaurar una marca mientras exista un valor vivo que dependa del espacio posterior a ella.

La operación de restauración recupera espacio en bloque; no ejecuta un `free` por objeto. Los recursos externos que requieran cierre sí tendrán destructores explícitos internos, ejecutados exactamente una vez antes de restaurar.

#### 2.5.4 Valores que crecen y estabilidad de direcciones

Una pila lineal no puede permitir que un buffer cambie de dirección mientras existan referencias internas válidas hacia él. Por ello:

- Los objetos de tamaño fijo y los temporales son candidatos directos a bump allocation.
- Un objeto redimensionable tendrá descriptor estable y buffer en un segmento cuya reubicación esté controlada.
- Si el buffer puede crecer, el compilador deberá demostrar que no hay referencias interiores vivas durante la operación de crecimiento, o utilizar handles/indirección estable.
- La primera implementación podrá reservar un segmento mayor o promover el objeto a una owned data stack; no debe introducir realloc silencioso que invalide referencias.
- No se impondrá todavía la restricción Goose de «un único valor redimensionable en la cima de cada pila». Se evaluará como posible optimización, no como regla del lenguaje.

Los handles internos podrán incluir propietario y generación para detectar errores durante desarrollo y pruebas. No sustituyen al análisis estático ni justifican referencias colgantes.

#### 2.5.5 Escape y transferencia de propiedad

El análisis trabajará sobre el grafo de valores y propietarios, no solo sobre el binding que recibe una asignación:

- Si un valor se devuelve, su almacenamiento pasa al propietario de retorno antes de destruir el frame.
- Si un valor se inserta en una colección que escapa, se analiza transitivamente su contenido.
- Si una closure escapa, su entorno y todas las capturas necesarias sobreviven con ella.
- Si varias closures comparten capturas mutables, comparten el mismo entorno lógico; la promoción mueve la propiedad del entorno/grupo, no clona las capturas.
- Las colecciones mantienen la semántica de valor actual: una copia debe preservar la independencia de sus buffers y elementos, sin duplicar semánticamente la identidad de los entornos capturados.
- Una lectura de vida desconocida nunca se asigna a una lane más corta por conjetura.

La transferencia debe preservar la semántica observable. Cuando sea seguro, se moverá el bloque completo o se cambiará el propietario del segmento. Cuando no lo sea, se hará una copia profunda conforme a la semántica de valor existente. La elección será explícita en el plan.

#### 2.5.6 Reutilización del análisis de Go y garantías de Rust

De Go se adopta la idea de analizar estáticamente si un valor escapa y decidir su ubicación; no se adopta su GC como requisito del runtime de HackLang.

De Rust se adoptan las garantías de propiedad, vida y exclusividad de mutación, pero no se trasladará al usuario la obligación de escribir anotaciones `&`, `&mut` o lifetimes. HackLang inferirá las restricciones internas. Si no puede demostrar seguridad, ampliará la vida del propietario o rechazará el caso cuando la ampliación no sea posible.

De Goose se adopta la planificación de varias pilas de datos, bump allocation y restauración por ámbito. No se copia su conjunto de restricciones ni se presupone que una única pila por ámbito sea suficiente para closures, recursividad y objetos redimensionables de HackLang.

#### 2.5.7 Invariantes que el plan deberá demostrar

Antes de que el backend consuma el plan, cada programa deberá satisfacer:

1. Cada objeto dinámico tiene exactamente un propietario de almacenamiento.
2. Toda referencia apunta a un objeto cuyo propietario vive al menos tanto como la referencia.
3. Ninguna restauración de cursor invalida un valor vivo.
4. Cada destructor se ejecuta como máximo una vez y antes de recuperar su almacenamiento.
5. Una transferencia de propietario conserva los valores y aliasing que exige la semántica actual.
6. La recursividad crea estado de pila independiente por activación.
7. Los errores y saltos no locales ejecutan la misma limpieza que una salida normal.
8. Los fallos de reserva y los desbordamientos de tamaño producen errores controlados.

El planificador deberá poder explicar, para cada binding y temporal, su clase de almacenamiento, lane, propietario, ámbito de vida y motivo de promoción. Esta explicación será la base de una futura orden `hl explain-memory`.



### 2.6.3 Contrato de limpieza y estado real de los backends

La inspección del código existente identifica dos mecanismos parciales que deben converger conceptualmente, sin crear un tercer subsistema:

- `Region::track<T>` devuelve ahora un `RegionFinalizerId` estable. `RegionTable::transfer_finalizer` mueve el registro (sin duplicarlo) a un ancestro, rechazando destinos de vida más corta; al cerrar el origen no se destruye y el destino ejecuta el destructor al cerrarse. La identidad es lógica y local al prototipo Rust, no un handle compartido con C.
- Las marcas de región registran una secuencia local de finalizadores, por lo que retirar o transferir una entrada anterior no invalida el límite de restauración.
- `HlEnv` mantiene una lista enlazada de registros C con callback y contexto. `hl_env_free` ejecuta los callbacks en orden LIFO; las asignaciones ordinarias usan el mismo mecanismo mediante un callback que llama a `free`.
- El generador C crea y libera entornos en bloques, y emite limpieza explícita antes de los saltos `break` y `continue`. La salida normal del bloque también libera el entorno. `hl_fail` limpia la cadena de entornos activos antes de terminar con `exit(1)`; sigue siendo un error fatal, no una excepción recuperable.

Por tanto, el runtime C ya tiene un registro genérico de callbacks, pero el prototipo `Region` y el `OwnershipLedger` no están conectados a él ni existe todavía paridad de ciclo de vida entre backends.

#### Contrato común propuesto

La futura integración utilizará un único concepto backend-neutral de registro de limpieza, asociado al token de propiedad ya definido:

1. **Acquire:** al crear un recurso que requiera destrucción, registrar una acción de limpieza en el propietario actual. La identidad del registro será estable durante su vida.
2. **Move:** transferir el registro existente al propietario nuevo; no registrar una segunda destrucción ni duplicar el recurso.
3. **Promote:** transferir el registro a un propietario cuya vida mayor haya sido demostrada por el análisis. El registro conserva el mismo token.
4. **Drop:** desactivar/consumir el registro exactamente una vez. Un registro consumido no puede volver a ejecutarse.
5. **Scope exit:** ejecutar los registros activos en orden inverso de adquisición y solo después recuperar el almacenamiento de la región.
6. **Non-local exit:** `return`, `break`, `continue` y error deben recorrer los mismos frames abandonados que la salida normal, respetando el destino del salto.
7. **Failure safety:** los fallos durante la reserva o la ejecución no pueden provocar una segunda destrucción. La política para fallos dentro de un destructor deberá definirse explícitamente; no se asumirá que un callback puede abortar la limpieza restante.

El ledger seguirá siendo el verificador del estado lógico; no será por sí mismo el registro físico de callbacks. El backend Rust y el backend C consumirán la misma secuencia de acciones del plan, adaptándola a sus mecanismos de almacenamiento, sin volver a inferir propiedad por separado.

#### Secuencia de integración y bloqueos

La implementación se hará por etapas verificables:

- Primero, definir un registro/stack de limpieza con identidad estable, desactivación y transferencia, y probar orden LIFO, transferencia y doble consumo en Rust.
- El runtime C ya implementa el registro genérico. Su transferencia solo permite mover un registro desde un entorno a uno de sus ancestros: el runtime puede demostrar esa relación léxica, pero no vidas arbitrarias. La prueba verifica también que una transferencia inversa hacia un ámbito más corto se rechaza sin perder el registro.
- Sustituir la terminación directa de `hl_fail` por propagación estructurada del error y limpieza de los entornos activos. No se añadirá un `longjmp` improvisado: debe preservarse el estado de los saltos generados y evitar saltarse destructores.
- Finalmente, generar y probar los mismos casos de salida en ambos backends: salida normal, anidamiento, `return`, `break`, `continue`, error y transferencia/promoción.

**Avance parcial en runtime C:** `HlAllocation` se ha convertido en un registro genérico de limpieza con callback y contexto. Las asignaciones ordinarias se registran mediante `hl_env_defer` usando un callback que libera su bloque; por tanto, la liberación de memoria y los futuros destructores comparten una sola pila LIFO. Se exponen además `hl_env_transfer`, que mueve un registro existente desde un entorno a un ancestro (y rechaza transferencias a vidas más cortas), y `hl_env_cancel`, que lo retira sin ejecutar el callback cuando el llamador ya ha resuelto la propiedad. `hl_env_free` consume la pila y ejecuta cada callback antes de liberar su nodo. El generador incluye una prueba estructural de que esta API forma parte del runtime emitido. Además, una prueba nativa compila y ejecuta el runtime C aislado: verifica el orden LIFO, la transferencia del mismo registro entre dos entornos y la cancelación sin ejecutar el callback.

**Avance en errores del runtime C:** `hl_fail` recibe ahora el entorno activo y recorre su cadena de padres, liberando cada entorno y ejecutando sus callbacks LIFO antes de terminar el proceso con código 1. La prueba nativa fuerza un error en un entorno hijo y utiliza un handler de salida para verificar que se limpiaron hijo y padre, en el orden esperado, antes de finalizar. Esto resuelve la limpieza de errores fatales del subconjunto C actual; no introduce excepciones recuperables ni propagación de errores al lenguaje.


### Arenas bump por entorno léxico (runtime C)

- El runtime C incorpora bloques `HlArenaBlock` por `HlEnv` y asignación bump alineada a `max_align_t`.
- Las asignaciones ordinarias de strings y bindings consumen el bloque actual; si no cabe el objeto, se reserva otro bloque (mínimo 4096 bytes o el tamaño solicitado).
- Al cerrar el entorno se ejecutan primero los finalizadores explícitos LIFO y después se liberan todos sus bloques de arena.
- El registro `hl_env_defer` se conserva separado para recursos con finalizador; ya no se crea una entrada de limpieza por cada asignación interna de la arena.
- La promoción entre ámbitos sigue siendo conservadora por copia al entorno propietario del binding. No se transfieren punteros entre arenas.
- Esto implementa arenas regionales bump en el runtime C para el subconjunto actual. `MemoryPlan.scope_stacks` elige las lanes físicas por profundidad y el backend las materializa en el C generado; los slots `Inline` del módulo también se conectan a `HlFrame`. Los frames/slots de activaciones de funciones todavía no se emiten porque las llamadas nativas siguen deshabilitadas.


### Frame de activación (runtime C)

El runtime incorpora una API mínima `HlFrame` con almacenamiento de slots proporcionado por el llamador. Cada activación mantiene su propio array `HlFrameSlot`, separado de los frames anidados o recursivos. Los slots registran si han sido inicializados; lectura de slots no inicializados, índices fuera de rango y escrituras inválidas fallan de forma controlada. La API no reserva memoria por sí misma: el compilador debe calcular y emitir el almacenamiento y conectar `BindingSlotPlan` antes de que el backend la use. Por tanto, esta entrega implementa el contrato/runtime de frames, no la asignación física automática desde `MemoryPlan`.


### Materialización del layout de frames

`frame_layout(program, memory_plan)` valida el plan y lo convierte en vectores densos por `DataStackId`. Cada posición contiene el `BindingId` asignado o queda vacía; el tamaño de cada vector coincide con `stack_slot_counts`. Esto proporciona una representación backend-neutral para emitir arrays C o almacenamiento equivalente en Rust. No conecta todavía esos vectores con las lecturas/escrituras de variables del generador, que siguen usando el entorno dinámico por nombre.

 
### Integración inicial del FrameLayout en C

El backend C materializa un `HlFrame` por activación de `main` y enruta los bindings clasificados como `Inline` a los slots físicos de `MemoryPlan`. Las lecturas y escrituras usan accesores comprobados (`hl_frame_read_or_fail` / `hl_frame_write_or_fail`). Los bindings dinámicos `ScopeRegion` y `OwnedRegion` continúan usando `HlEnv` y sus arenas/promoción conservadora, evitando almacenar punteros de ámbito corto en un frame cuya vida se extiende más allá de la región. Esta es una integración parcial deliberada: las funciones HackLang y los frames recursivos se conectarán cuando exista lowering de funciones; no se debe inferir que ya hay asignación de slots para todos los valores.
