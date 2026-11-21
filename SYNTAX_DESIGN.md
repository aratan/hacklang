# HackLang: diseño de sintaxis orientada a agentes

## Objetivo

Minimizar el coste total de los programas generados por agentes: tokens de entrada + tokens de salida + errores de compilación + iteraciones de reparación. No se optimiza el número de caracteres por sí solo.

La sintaxis debe tener:
- Una sola forma canónica por operación.
- Gramática determinista, sin backtracking ni decisiones dependientes de espacios o saltos de línea.
- Pocas palabras reservadas y delimitadores reutilizables con reglas claras.
- Diagnósticos estructurados y estables para reparación automática.
- Lectura humana suficiente para revisar código generado.
- Compatibilidad: la sintaxis actual no se elimina durante la transición.

## Criterio de diseño

No asumir que un símbolo corto equivale a un token barato. Los tokenizadores BPE dividen palabras y símbolos de manera distinta. Toda decisión debe medirse con tokenizadores reales (al menos cl100k_base y o200k_base; añadir tokenizadores de los modelos locales objetivo cuando estén disponibles).

La métrica principal será:

`coste = tokens(prompt+fuente+diagnóstico+reparación) × iteraciones`

También se medirá Pass@1, tasa de compilación, tasa de éxito funcional y legibilidad en revisión. Una sintaxis que ahorre tokens pero genere más errores puede salir más cara.

## Sintaxis compacta experimental v0 (implementación inicial)

Ya existe un modo explícito de parser compacto, activado con `hl --compact archivo.hl`. No cambia el modo legacy por defecto.

Implementado y cubierto por pruebas:

- Declaración compacta `name:=expr`.
- Asignación existente `name=expr`.
- Bloques `{...}` para bloques independientes, `if`, `else`, `while` y cuerpos de lambdas.
- Condiciones sin paréntesis: `if cond{...}` y `while cond{...}`.
- Mapas compactos `@{key:value}`.
- Punto y coma opcional cuando la gramática puede reconocer el siguiente inicio de sentencia.
- Formateador AST → sintaxis canónica compacta, disponible con `--format`; puede leer sintaxis legacy o compacta y emite compacta.
- Compatibilidad del parser legacy con sus paréntesis y declaraciones `let`.

Se mantienen las palabras clave `print`, `return`, `break` y `continue`; no se han introducido abreviaturas que colisionen con el ternario o con operadores. La implementación compacta es una primera versión experimental, no todavía la sintaxis predeterminada.

### Diferencias de delimitadores entre modos

| Construcción | Legacy (predeterminado) | Compacto (`--compact`) |
|---|---|---|
| Declaración | `let nombre = expr;` | `nombre:=expr` |
| Bloque | `( sentencias )` | `{ sentencias }` |
| Mapa | `@(clave: valor)` | `@{clave:valor}` |
| Lambda de expresión | `|a,b| (expr)` | `|a,b|expr` |
| Condición | `if (cond) (...)` / `while (cond) (...)` | `if cond{...}` / `while cond{...}` |

No mezclar delimitadores de ambos modos en un mismo archivo salvo que el parser documente expresamente esa construcción. En particular, `@{...}` es la forma compacta de mapa y no debe usarse en ejemplos legacy.

## Siguientes reducciones a evaluar

- Impresión abreviada: no asignar `?` hasta resolver la interacción inequívoca con el operador ternario; alternativa segura a estudiar: una palabra clave corta nueva, medida con tokenizadores.
- Retorno: mantener `return`; `^` ya es operador y no se reutilizará sin rediseñar la precedencia y demostrar compatibilidad.
- Un mapa compacto usa `@{}`; arrays mantienen `[]`.

## Propuesta compacta (evolución propuesta)

Ejemplo completo:

```hacklang
n:=10
sum:=0
while n>0{
  sum=sum+n
  n=n-1
}
?sum

add:=|a,b|a+b
?add(2,3)

fact:=|n|{
  if n<2{return 1}
  return n*fact(n-1)
}
?fact(5)
```

Formas canónicas propuestas:

| Operación | Forma compacta | Forma actual |
|---|---|---|
| Declarar | `name:=expr` | `let name = expr;` |
| Asignar | `name=expr` | `name = expr;` |
| Imprimir | `?expr` | `print expr;` |
| Retornar | `return expr` inicialmente; evaluar `^expr` solo si mejora la métrica | `return expr;` |
| Bloque | `{ stmts }` | `( stmts )` |
| Condicional | `if cond{...}else{...}` | `if (cond) (...) else (...)` |
| Bucle | `while cond{...}` | `while (cond) (...)` |
| Lambda | `|a,b|expr` | `|a, b| expr` |
| Array | `[a,b]` | `[a, b]` |
| Mapa | `@{k:v}` | `@(k:v)` |

La tabla define un objetivo de experimento, no una promesa de compatibilidad ni una gramática ya implementada. En particular, no se introducirá `^expr` como retorno hasta resolver su colisión con el operador actual de desigualdad; tampoco se asignará `?` a impresión sin probar su interacción con el ternario.

## Reglas que no se deben sacrificar

1. No usar indentación como sintaxis.
2. No hacer que los saltos de línea cambien el significado.
3. No permitir múltiples alias permanentes para la misma operación en la sintaxis canónica.
4. Mantener precedencia de operadores documentada y testeada.
5. Mantener tipos explícitos en runtime; el análisis estático puede ser gradual, pero no debe inventar tipos.
6. Los errores deben incluir código estable, fase, posición y mensaje breve; el modo JSON será la interfaz preferida por agentes.
7. Los programas compactos deben poder formatearse a una vista legible sin cambiar su significado.

## Plan de implementación

1. Congelar la gramática actual como sintaxis legacy y añadir pruebas de compatibilidad.
2. Implementar la sintaxis compacta detrás de una versión/modo explícito, sin cambiar silenciosamente programas existentes.
3. Añadir un formateador que convierta AST a la forma canónica compacta.
4. Añadir diagnósticos JSON con códigos estables (primera versión implementada); propagar spans por lexer/parser/AST para completar rangos y estructurar también errores léxicos y sintácticos.
5. Crear un corpus de tareas equivalentes en HackLang, Python y Lua. Medir tokens con el mismo tokenizador y modelos, y Pass@1 con criterios funcionales idénticos.
6. Promover la sintaxis compacta a predeterminada solo si reduce el coste total y no empeora de forma significativa la tasa de éxito.

## Estado actualizado del proyecto

La CI ejecuta todos los archivos `examples/*.hl` usando el modo correspondiente indicado por cada prueba. Los ejemplos que dependen de funciones aún no implementadas (`read_file`, `write_file`, `exec`) se conservan fuera de `examples/`, en `prototypes/unsupported/`, y no definen la API del lenguaje.

Ya se han cerrado dos limitaciones iniciales del comprobador:

- Los retornos explícitos de funciones y lambdas se incorporan a la inferencia; ramas con tipos de retorno incompatibles generan diagnóstico.
- Los arrays conservan el tipo inferido de sus elementos y los mapas conservan los tipos de clave y valor. La lectura por índice y las asignaciones por índice comprueban esos tipos.
- Los elementos heterogéneos de arrays y las claves o valores incompatibles se notifican sin bloquear la ejecución.

Pendiente:

- Inferencia de tipos de parámetros a partir del cuerpo y de los argumentos de llamada.
- Diagnósticos JSON con código estable y severidad ya implementados para el análisis estático; el rango se devuelve como `null` porque el AST todavía no conserva spans. Los errores léxicos y sintácticos aún usan el canal textual. El lexer ya expone `tokenize_spanned()` con offsets y línea/columna Unicode-aware, manteniendo `tokenize()` compatible. El parser todavía consume `Vec<Token>` y el AST no conserva spans; falta una ruta compatible para propagar posiciones hasta los diagnósticos.
- Benchmark reproducible de tokens, compilación y Pass@1 frente a Python y Lua.
- Evaluar si merece la pena abreviar `print` y otras palabras clave sin crear ambigüedad.

Estos puntos son trabajo pendiente, no funcionalidades ya terminadas. La sintaxis compacta está implementada detrás de `--compact` y dispone de formateador `--format`; sigue siendo experimental y no sustituye la sintaxis legacy por defecto.
