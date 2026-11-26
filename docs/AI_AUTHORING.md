# AI authoring profile

Estado: guía de uso para la sintaxis compacta existente. No añade sintaxis ni cambia la gramática.

## Objetivo

HackLang está pensado para que un modelo genere programas pequeños, inequívocos y baratos de transmitir. La prioridad es minimizar tokens **sin aumentar ambigüedad ni errores**. El ahorro real depende del tokenizer del modelo que consuma el código; contar caracteres no basta.

## Perfil recomendado para generación

Usar el modo compacto explícitamente:

```sh
hl run --compact program.hl
hl build --compact program.hl -o program
hl run --compact --format program.hl
```

El formato canónico compacto se puede generar con `--format`. No crear minificadores externos: podrían eliminar separadores necesarios o alterar cadenas.

Ejemplo:

```hacklang
n:=10
sum:=0
while n>0{
  sum=sum+n
  n=n-1
}
print sum
```

## Reglas para modelos generadores

1. Una sentencia por línea cuando no haya un separador inequívoco.
2. Omitir espacios alrededor de operadores solo cuando el lexer pueda separar tokens sin cambiar su significado.
3. No omitir delimitadores de bloques, llamadas, arrays o mapas.
4. Usar paréntesis cuando una expresión pueda tener más de una lectura.
5. No inventar operadores, abreviaturas, nombres implícitos ni conversiones.
6. Mantener nombres cortos pero semánticos; evitar colisiones y reutilización confusa.
7. No generar construcciones que el backend nativo aún rechaza. El intérprete soporta más características que `hl build`.
8. Si una construcción no está soportada, preferir un error explícito a una traducción aproximada.

## Sintaxis compacta actual

| Construcción | Forma compacta |
|---|---|
| Declaración | `x:=expr` |
| Asignación | `x=expr` |
| Bloque | `{ ... }` |
| Condición | `if cond{ ... }` |
| Bucle | `while cond{ ... }` |
| Lambda | `f:=|x|{ ... }` |
| Mapa | `m:=@{key:value}` |
| Impresión | `print expr` |
| Retorno | `return expr` |
| Salto de bucle | `break`, `continue` |

Las palabras clave se mantienen completas deliberadamente: abreviarlas ahorraría poco y añadiría reglas especiales al lexer y al modelo generador.

## Contrato de inequívocidad

- El parser debe producir un único AST para cada programa aceptado.
- Un programa mal formado debe fallar con diagnóstico y ubicación, nunca reinterpretarse silenciosamente.
- La forma compacta y la legacy deben conservar la misma semántica cuando representan el mismo programa.
- El formateador es la referencia para la salida canónica; no se deben mantener reglas de minificación paralelas.
- La compilación nativa debe rechazar las variantes no implementadas antes de emitir un ejecutable incorrecto.

## Métrica de ahorro

No se declarará un porcentaje de ahorro hasta medirlo con tokenizadores reales. La evaluación deberá comparar, sobre un corpus idéntico:

- tokens de entrada en formato legacy;
- tokens de entrada en formato compacto canónico;
- tokens de instrucciones y contexto requeridos para que un modelo genere cada variante;
- tasa de parseo correcto y equivalencia semántica;
- tamaño del ejecutable y tiempo de compilación, como métricas separadas del coste de tokens.

La métrica principal será el coste total de tokens para producir un programa correcto, no la longitud visual del código.
