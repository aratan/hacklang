# Backend nativo AOT con Cranelift

Estado: plan de implementación y spike aislado. No representa todavía un backend HackLang funcional.

## Objetivo medible inicial

Generar un objeto nativo para el target anfitrión desde Rust, sin generar C y sin ejecutar `cc`, GCC ni Clang. Este primer objeto de prueba no demuestra todavía que HackLang compile programas reales.

## Dependencias y versiones

Usar crates Cranelift alineados en una misma versión estable. La versión consultada el 27-11-2026 es 0.138.0:
- `cranelift-codegen`
- `cranelift-frontend`
- `cranelift-module`
- `cranelift-native`
- `cranelift-object`

Cranelift soporta x86-64, AArch64, s390x y riscv64. La primera implementación se limita al target anfitrión; no añadir cross-compilation en el spike.

## Estado del trabajo

La rama `feat/cranelift-object-backend` contiene el primer spike C0: dependencias Cranelift alineadas en 0.138.0 y una función aislada que emite un objeto nativo con `main() -> i32 { 0 }`. El módulo no está conectado a `hl build`, no consume HIR y no afirma compilar programas HackLang. Su test comprueba que el objeto se genera sin invocar un compilador C y valida el magic ELF en Linux.

## Estado del trabajo

- C0: implementado como spike de emisión de objeto nativo con `main() -> i32`; CI de GitHub pasó en el commit `6e1ab1b`.
- C1 parcial: `emit_numeric_expression_object` consume directamente `hir::Expression` y baja literales numéricos finitos y operadores `+`, `-`, `*`, `/` a Cranelift IR. Incluye test de HIR `6 * 7` que verifica la emisión de objeto. Aún no compila un `Program` completo, no evalúa el resultado ni cubre variables, comparaciones o control de flujo. CI del nuevo commit pendiente.

## Fases

### C0 — Prueba de emisión de objeto
- Inicializar ISA nativa.
- Crear `ObjectModule`.
- Emitir una función `main() -> i32` que retorne cero.
- Escribir el objeto mediante la API de `cranelift-object`.
- Comprobar magic/formato, arquitectura y símbolo exportado usando el lector de objetos.
- El test no invoca linker ni compilador C.

### C1 — HIR mínimo
- Definir contrato de lowering HIR→Cranelift IR.
- Implementar Number, Bool, variables locales, aritmética y comparaciones.
- Implementar bloques, if/while y retornos.
- Comparar salida y errores con el intérprete.
- Reutilizar HIR, spans y checker sin hacer que el checker sea obligatorio.

### C2 — Runtime
- Sustituir llamadas generadas a runtime C por funciones Rust con ABI C estable o runtime compilado con el propio backend.
- Migrar traps, salida y valores etiquetados.
- Después strings, arrays y maps.
- Funciones/closures solo tras fijar ABI, calling convention y representación de entornos.
- No modificar memoria/escape analysis: adaptar el runtime y el lowering al plan existente.

### C3 — Ejecutable
- Enlazar el objeto con runtime y startup code.
- Primera etapa puede usar linker del sistema; reportar esa dependencia de forma explícita.
- Evaluar linker integrado o distribución de una toolchain mínima en fase posterior.
- Probar ELF primero en Linux x86-64; PE y Mach-O son objetivos separados.

## Reglas

- El backend C sigue siendo fallback.
- No activar Cranelift como backend por defecto hasta superar paridad diferencial y sanitizers.
- No añadir una dependencia que no esté usada por código y tests.
- No llamar al trabajo «compilador autónomo completo» hasta que el runtime y el proceso de enlace también estén resueltos.
