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

## Propuesta compacta (objetivo v0.2; aún no implementada)

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
4. Añadir diagnósticos JSON con códigos y rangos de fuente.
5. Crear un corpus de tareas equivalentes en HackLang, Python y Lua. Medir tokens con el mismo tokenizador y modelos, y Pass@1 con criterios funcionales idénticos.
6. Promover la sintaxis compacta a predeterminada solo si reduce el coste total y no empeora de forma significativa la tasa de éxito.

## Pendientes detectados en la revisión

- El comprobador actual infiere tipos básicos, pero los bloques de función devuelven `unknown`; no infiere de forma completa los retornos explícitos.
- Los tipos de arrays/mapas no conservan tipos de elemento/clave/valor.
- No hay diagnósticos JSON ni posiciones de origen en el AST.
- No existe un formateador ni un benchmark de tokens/pass@1.
- El lexer/parser actual tiene construcciones heredadas que conviene mantener mientras se introduce la sintaxis nueva.

Estos puntos son trabajo pendiente, no funcionalidades ya terminadas.
