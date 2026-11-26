# HIR: representación intermedia de alto nivel

Estado: **HIR inicial implementada y consumida por el backend C para el subconjunto nativo; expansión de cobertura pendiente**.

Este documento fija la frontera entre el frontend de HackLang y sus consumidores (intérprete, análisis y backend nativo). Su objetivo es permitir que el lenguaje crezca sin duplicar semántica ni atar el compilador al AST concreto del parser.

## 1. Decisión arquitectónica

Pipeline objetivo:

```text
source
  -> lexer (tokens con SourceSpan)
  -> parser legacy/compacto
  -> AST
  -> validación semántica y resolución de nombres
  -> HIR
  -> análisis de tipos, ámbitos, efectos y escapes
  -> MIR / lowering de control y memoria
  -> backend C (primero) -> compilador C -> ejecutable
```

Reglas:

- Un solo lexer, parser y AST. No crear un segundo frontend para el compilador.
- HIR se construye desde el AST; nunca vuelve a analizar texto fuente.
- El AST conserva la estructura y sintaxis de origen. HIR expresa significado normalizado y referencias resueltas.
- El backend no decide reglas del lenguaje: consume HIR validada y falla explícitamente ante operaciones no soportadas.
- Durante la primera migración, el evaluador AST sigue siendo la implementación de referencia. La paridad se verifica con pruebas diferenciales. No se sustituye hasta que un consumidor HIR haya demostrado equivalencia.
- La sintaxis legacy y compacta debe producir HIR equivalente para programas semánticamente equivalentes.

## 2. Qué debe contener HIR

HIR debe ser independiente del texto original y estable frente a cambios de sintaxis. Como mínimo:

### Identidad y ubicaciones

- IDs estables dentro de un módulo: `ExprId`, `StmtId`, `ScopeId`, `FunctionId`, `BindingId`.
- `SourceSpan` opcional en cada nodo, propagado desde `SpannedToken`; los nodos sintéticos pueden no tener span y deben señalar el nodo origen cuando exista.
- Tabla de símbolos: cada lectura/escritura referencia un `BindingId`, no vuelve a resolver nombres por cadenas.
- Tabla de ámbitos con padre, clase de ámbito y bindings declarados.

### Expresiones

- Literales Number/Bool/String.
- Lectura de binding y asignación.
- Operaciones unarias/binarias y ternario.
- Array, Map e indexación.
- Lambda/función y llamada.
- Bloque como expresión, conservando su valor de retorno.
- Builtins como operaciones identificadas, no como nombres mágicos dispersos en cada backend.

### Sentencias y flujo

- Declaración, asignación, asignación indexada, print.
- Bloque, if, while.
- Return, break y continue con destino de control resuelto.
- Salidas explícitas para errores semánticos; no convertir errores en valores por defecto.

### Metadatos de análisis

- Tipo conocido o Unknown, sin fingir que Unknown es seguridad estática.
- Mutabilidad y modo de captura de cada binding.
- Ámbito de definición y ámbito de uso.
- Efectos observables: salida, mutación, fallo, llamada y acceso a colecciones.
- Información de escape/vida solo después del análisis correspondiente; no inferirla durante el parseo.

## 3. Normalización semántica

HIR debe fijar y representar, no reinventar, las reglas del evaluador actual:

- Orden de evaluación de operandos y argumentos.
- Ámbito exacto de declaraciones y asignaciones.
- Captura léxica por identidad y visibilidad de mutaciones.
- Semántica de truthiness y operadores.
- Comportamiento de return/break/continue.
- Orden de evaluación de elementos y pares de mapas.
- Errores de tipo, división por cero, nombre no definido e índices inválidos.

Antes de declarar paridad, cualquier comportamiento accidental o inconsistente detectado en el evaluador se documentará y corregirá como cambio del lenguaje, con pruebas. El backend no debe copiar silenciosamente errores históricos.

## 4. HIR no es MIR

HIR conserva operaciones de alto nivel (por ejemplo, `ArrayLiteral`, `Call`, `Closure`, `While`). MIR será una fase posterior que hará explícitos:

- bloques básicos y terminadores de control;
- temporales y asignaciones SSA o una forma equivalente justificada;
- llamadas, retornos y saltos;
- slots de stack, regiones y operaciones de vida;
- puntos de asignación, promoción y destrucción.

No implementar SSA, optimizador ni asignador de registros para la primera demo. MIR se introduce cuando una necesidad concreta del backend o de escape analysis lo justifique.

## 5. Contrato de memoria

HIR identifica ámbitos, bindings, capturas y relaciones de uso; **no libera memoria ni elige punteros C**.

La estrategia se decide después:

1. Resolver identidad y mutabilidad de bindings.
2. Calcular vida y escapes de valores.
3. Asignar almacenamiento: inline, stack, región local o región promovida.
4. Insertar destrucción/promoción en MIR.
5. Validar que ninguna referencia sobrevive a su propietario.

Condiciones obligatorias:

- Una closure escapada conserva su entorno y las capturas mutables mantienen identidad compartida.
- Arrays/mapas anidados conservan los valores referenciados que escapan.
- Ciclos de closures y recursividad no pueden depender de GC o reference counting universal.
- Si el análisis no demuestra que una ubicación corta es segura, elige una vida más larga.
- No se introduce sintaxis de memoria en el lenguaje para compensar una limitación del compilador.

## 6. Migración incremental

### Fase H0 — contrato y pruebas

- Mantener AST y evaluador actuales.
- Crear pruebas de semántica con resultados y errores esperados.
- Añadir pruebas de equivalencia legacy/compacto.
- Propagar spans del AST cuando se modifiquen sus constructores, sin volver a tokenizar.

### Fase H1 — estructura mínima

- Crear módulo `hir` con IDs, tablas de ámbitos y nodos.
- Implementar lowering AST -> HIR para el subconjunto actual.
- Probar que todos los nodos AST del subconjunto se convierten o producen un diagnóstico explícito.
- No cambiar todavía la ruta de ejecución de `hl run` ni `hl build`.

### Fase H2 — primer consumidor

- [x] Migrar el generador C para consumir HIR en el subconjunto inicial.
- [x] Comparar ejecutables HIR con el intérprete en el corpus nativo de CI.
- [x] Mantener el rechazo temprano de nodos no soportados.
- [ ] Ampliar el corpus diferencial con shadowing, scopes anidados y todas las rutas de control.

### Fase H3 — análisis y expansión

- Añadir resolución de bindings, tipos, efectos y capturas.
- Incorporar arrays/mapas/funciones por familias, cada una con pruebas diferenciales.
- Introducir MIR cuando la gestión de control o memoria requiera una representación explícita.

### Fase H4 — migración del intérprete

Solo cuando HIR sea estable y cubra todo el lenguaje, evaluar migrar el intérprete a HIR. No se hará por estética ni para cumplir un diagrama.

## 7. Puertas de calidad

Cada fase debe cumplir:

- Compilación y suite completa verdes.
- Ningún nodo AST aceptado queda sin lowering definido.
- Pruebas de equivalencia de sintaxis legacy/compacta.
- Pruebas diferenciales de stdout, stderr, código de salida y errores.
- Diagnósticos con ubicación de origen cuando exista span.
- Ningún cambio de comportamiento no documentado.
- El backend rechaza antes de invocar al compilador C cualquier construcción que no soporte.

## 8. No objetivos

- No crear un segundo parser ni una gramática exclusiva del compilador.
- No duplicar el checker ni la evaluación de operadores dentro de C.
- No añadir anotaciones manuales de ownership, lifetimes o regiones.
- No implementar optimizaciones prematuras ni una MIR gigantesca antes de necesitarla.
- No prometer portabilidad a otras arquitecturas hasta definir y probar los backends.

## 9. Primer hito verificable

El hito H1/H2 no es «tener una estructura HIR bonita». Es convertir el subconjunto que ya compila a nativo en HIR, probar su cobertura y hacer que el backend C consuma HIR sin cambiar los resultados del corpus diferencial.


## 10. Estado de implementación (2026-11-22)

Actualizado tras la migración HIR→C del 22-11-2026.

Implementado en `src/hir.rs`:

- IDs tipados `ScopeId`, `BindingId`, `ExprId` y `StmtId`.
- Tabla de scopes con relación padre y clases Module, Block, Branch y Loop.
- Tabla de bindings y resolución léxica de lecturas/asignaciones al binding más cercano.
- Lowering AST → HIR para Number, Bool, String, lecturas, operadores binarios, ternario, let, asignación, print, bloques, if, while, break y continue.
- Rechazo explícito de expresiones/sentencias fuera del subconjunto inicial y de nombres no resueltos.
- Pruebas unitarias para IDs de bindings, shadowing, asignación, nombres no resueltos, control de bucles y scope compartido del while.

La compilación nativa ejecuta el lowering AST→HIR y `src/hir_native.rs` genera C directamente desde HIR. El generador AST duplicado se ha retirado de `src/native.rs`; este módulo se limita a orquestar lowering, toolchain y salida. La CI compara el resultado del intérprete y el binario nativo para el corpus de smoke test, que incluye shadowing y saltos de bucle desde ramas.

Limitaciones deliberadas de esta primera entrega:

- No soporta todavía lambdas, llamadas, arrays, mapas, indexación, return ni asignación indexada en HIR.
- No incorpora spans en nodos HIR; se añadirá al resolver la propagación de spans del AST, sin volver a tokenizar.
- La resolución de nombres es léxica y secuencial; la semántica especial de autorreferencia de closures y builtins se añadirá con la fase de funciones.
- El checker de tipos y el evaluador de referencia no se han migrado.
