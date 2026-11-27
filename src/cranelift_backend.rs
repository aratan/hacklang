//! Isolated AOT object-emission spike. This module deliberately does not
//! participate in `hl build` yet: it proves Cranelift can emit a native
//! object from Rust without generating C or invoking a system C compiler.

use std::path::Path;

use cranelift_codegen::ir::{types, AbiParam, InstBuilder};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

/// Emit a minimal native object exporting `main() -> i32` and returning zero.
///
/// This is a backend/toolchain smoke test, not yet a HackLang HIR compiler.
pub fn emit_probe_object(output: &Path) -> Result<(), String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|error| format!("No se pudo configurar Cranelift: {error}"))?;
    let flags = settings::Flags::new(flag_builder);

    let isa_builder = cranelift_native::builder()
        .map_err(|error| format!("Target nativo no soportado: {error}"))?;
    let isa = isa_builder
        .finish(flags)
        .map_err(|error| format!("No se pudo inicializar la ISA nativa: {error}"))?;

    let object_builder = ObjectBuilder::new(
        isa,
        b"hacklang-probe",
        default_libcall_names(),
    )
    .map_err(|error| format!("No se pudo crear el módulo objeto: {error}"))?;
    let mut module = ObjectModule::new(object_builder);

    let mut signature = module.make_signature();
    signature.returns.push(AbiParam::new(types::I32));
    let function_id = module
        .declare_function("main", Linkage::Export, &signature)
        .map_err(|error| format!("No se pudo declarar main: {error}"))?;

    let frontend_config = module.target_config();
    let mut context = module.make_context();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let zero = builder.ins().iconst(types::I32, 0);
        builder.ins().return_(&[zero]);
        builder.finalize(frontend_config);
    }

    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("No se pudo generar main: {error}"))?;
    module.clear_context(&mut context);

    let object = module.finish().object.write()
        .map_err(|error| format!("No se pudo serializar el objeto nativo: {error}"))?;
    std::fs::write(output, object)
        .map_err(|error| format!("No se pudo escribir {}: {error}", output.display()))
}



/// Lower a straight-line numeric HIR statement sequence to a native object.
///
/// Supported statements: numeric let, assignment to an existing numeric
/// binding, nested blocks, and return. Every binding is represented by an
/// SSA Cranelift value; branches, loops, calls and I/O remain unsupported.
pub fn emit_numeric_statements_object(
    statements: &[crate::hir::Statement],
    output: &Path,
) -> Result<(), String> {
    use crate::hir::{BindingId, ExprKind, Statement, StmtKind};
    use std::collections::HashMap;

    fn lower_expr(
        builder: &mut FunctionBuilder<'_>,
        expression: &crate::hir::Expression,
        values: &HashMap<BindingId, cranelift_codegen::ir::Value>,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        use crate::hir::BinaryOp;
        match &expression.kind {
            ExprKind::Number(value) if value.is_finite() => Ok(builder.ins().f64const(*value)),
            ExprKind::Number(_) => Err("NaN e infinito no backend Cranelift aún no están soportados".into()),
            ExprKind::Read(binding) => values.get(binding).copied()
                .ok_or_else(|| format!("Binding numérico {:?} no inicializado", binding)),
            ExprKind::Binary { op, left, right } => {
                let left = lower_expr(builder, left, values)?;
                let right = lower_expr(builder, right, values)?;
                match op {
                    BinaryOp::Add => Ok(builder.ins().fadd(left, right)),
                    BinaryOp::Sub => Ok(builder.ins().fsub(left, right)),
                    BinaryOp::Mul => Ok(builder.ins().fmul(left, right)),
                    BinaryOp::Div => Ok(builder.ins().fdiv(left, right)),
                    _ => Err("Comparaciones aún no soportadas en el lowering numérico".into()),
                }
            }
            _ => Err("Expresión no soportada en el lowering numérico".into()),
        }
    }

    fn lower_statements(
        builder: &mut FunctionBuilder<'_>,
        statements: &[Statement],
        values: &mut HashMap<BindingId, cranelift_codegen::ir::Value>,
    ) -> Result<Option<cranelift_codegen::ir::Value>, String> {
        for statement in statements {
            match &statement.kind {
                StmtKind::Let { binding, value } => {
                    let value = lower_expr(builder, value, values)?;
                    values.insert(*binding, value);
                }
                StmtKind::Assign { binding, value } => {
                    if !values.contains_key(binding) {
                        return Err(format!("Asignación a binding numérico {:?} no inicializado", binding));
                    }
                    let value = lower_expr(builder, value, values)?;
                    values.insert(*binding, value);
                }
                StmtKind::Block { statements, .. } => {
                    if let Some(value) = lower_statements(builder, statements, values)? {
                        return Ok(Some(value));
                    }
                }
                StmtKind::Return(expression) => {
                    return Ok(Some(lower_expr(builder, expression, values)?));
                }
                _ => return Err("Sentencia aún no soportada en el backend numérico Cranelift".into()),
            }
        }
        Ok(None)
    }

    let mut flag_builder = settings::builder();
    flag_builder.set("is_pic", "true").map_err(|error| error.to_string())?;
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = cranelift_native::builder().map_err(|error| error.to_string())?;
    let isa = isa_builder.finish(flags).map_err(|error| error.to_string())?;
    let object_builder = ObjectBuilder::new(isa, b"hacklang-hir-statements", default_libcall_names())
        .map_err(|error| error.to_string())?;
    let mut module = ObjectModule::new(object_builder);

    let mut signature = module.make_signature();
    signature.returns.push(AbiParam::new(types::F64));
    let function_id = module.declare_function("hl_entry", Linkage::Export, &signature)
        .map_err(|error| error.to_string())?;
    let frontend_config = module.target_config();
    let mut context = module.make_context();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let mut values = HashMap::new();
        let result = lower_statements(&mut builder, statements, &mut values)?
            .unwrap_or_else(|| builder.ins().f64const(0.0));
        builder.ins().return_(&[result]);
        builder.finalize(frontend_config);
    }
    module.define_function(function_id, &mut context).map_err(|error| error.to_string())?;
    module.clear_context(&mut context);
    let object = module.finish().object.write().map_err(|error| error.to_string())?;
    std::fs::write(output, object).map_err(|error| format!("No se pudo escribir el objeto: {error}"))
}

/// Lower a numeric HIR expression to a native object exporting
/// `hl_entry() -> f64`. The initial supported subset is deliberately small:
/// finite numeric literals and +, -, *, /. It is the first real HIR→Cranelift
/// path; it does not yet compile complete HackLang programs.
pub fn emit_numeric_expression_object(
    expression: &crate::hir::Expression,
    output: &Path,
) -> Result<(), String> {
    use crate::hir::{BinaryOp, ExprKind};

    fn lower(
        builder: &mut FunctionBuilder<'_>,
        expression: &crate::hir::Expression,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        match &expression.kind {
            ExprKind::Number(value) if value.is_finite() => {
                Ok(builder.ins().f64const(*value))
            }
            ExprKind::Number(_) => Err("NaN e infinito no backend Cranelift aún no están soportados".into()),
            ExprKind::Binary { op, left, right } => {
                let left = lower(builder, left)?;
                let right = lower(builder, right)?;
                match op {
                    BinaryOp::Add => Ok(builder.ins().fadd(left, right)),
                    BinaryOp::Sub => Ok(builder.ins().fsub(left, right)),
                    BinaryOp::Mul => Ok(builder.ins().fmul(left, right)),
                    BinaryOp::Div => Ok(builder.ins().fdiv(left, right)),
                    _ => Err("Operador no soportado por el lowering numérico Cranelift".into()),
                }
            }
            _ => Err("Expresión HIR no soportada por el lowering numérico Cranelift".into()),
        }
    }

    let mut flag_builder = settings::builder();
    flag_builder.set("is_pic", "true").map_err(|error| error.to_string())?;
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = cranelift_native::builder().map_err(|error| error.to_string())?;
    let isa = isa_builder.finish(flags).map_err(|error| error.to_string())?;
    let object_builder = ObjectBuilder::new(isa, b"hacklang-hir", default_libcall_names())
        .map_err(|error| error.to_string())?;
    let mut module = ObjectModule::new(object_builder);

    let mut signature = module.make_signature();
    signature.returns.push(AbiParam::new(types::F64));
    let function_id = module.declare_function("hl_entry", Linkage::Export, &signature)
        .map_err(|error| error.to_string())?;
    let frontend_config = module.target_config();
    let mut context = module.make_context();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let value = lower(&mut builder, expression)?;
        builder.ins().return_(&[value]);
        builder.finalize(frontend_config);
    }
    module.define_function(function_id, &mut context).map_err(|error| error.to_string())?;
    module.clear_context(&mut context);
    let object = module.finish().object.write().map_err(|error| error.to_string())?;
    std::fs::write(output, object).map_err(|error| format!("No se pudo escribir el objeto: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{emit_numeric_expression_object, emit_numeric_statements_object, emit_probe_object};

    #[test]
    fn lowers_numeric_hir_bindings_and_assignments_to_object() {
        use crate::hir::{BindingId, BinaryOp, ExprId, ExprKind, Expression, Statement, StmtId, StmtKind};
        let number = |id, value| Expression { id: ExprId(id), kind: ExprKind::Number(value) };
        let read = |id, binding| Expression { id: ExprId(id), kind: ExprKind::Read(BindingId(binding)) };
        let binary = |id, op, left, right| Expression {
            id: ExprId(id),
            kind: ExprKind::Binary { op, left: Box::new(left), right: Box::new(right) },
        };
        let statements = vec![
            Statement { id: StmtId(0), kind: StmtKind::Let { binding: BindingId(0), value: number(0, 6.0) } },
            Statement { id: StmtId(1), kind: StmtKind::Let {
                binding: BindingId(1),
                value: binary(3, BinaryOp::Add, read(1, 0), number(2, 7.0)),
            }},
            Statement { id: StmtId(2), kind: StmtKind::Assign {
                binding: BindingId(0),
                value: binary(6, BinaryOp::Mul, read(4, 1), number(5, 2.0)),
            }},
            Statement { id: StmtId(3), kind: StmtKind::Return(read(7, 0)) },
        ];
        let path = std::env::temp_dir().join(format!(
            "hacklang-cranelift-bindings-{}.o", std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        emit_numeric_statements_object(&statements, &path).expect("lower numeric bindings");
        let bytes = std::fs::read(&path).expect("read bindings object");
        assert!(!bytes.is_empty());
        #[cfg(target_os = "linux")]
        assert!(bytes.starts_with(b"\x7fELF"));
        std::fs::remove_file(path).expect("remove bindings object");
    }

    #[test]
    fn lowers_numeric_hir_expression_to_native_object() {
        use crate::hir::{BinaryOp, ExprId, ExprKind, Expression};

        let expression = Expression {
            id: ExprId(2),
            kind: ExprKind::Binary {
                op: BinaryOp::Mul,
                left: Box::new(Expression { id: ExprId(0), kind: ExprKind::Number(6.0) }),
                right: Box::new(Expression { id: ExprId(1), kind: ExprKind::Number(7.0) }),
            },
        };
        let path = std::env::temp_dir().join(format!(
            "hacklang-cranelift-hir-{}.o", std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        emit_numeric_expression_object(&expression, &path).expect("lower numeric HIR");
        let bytes = std::fs::read(&path).expect("read HIR object");
        assert!(!bytes.is_empty());
        #[cfg(target_os = "linux")]
        assert!(bytes.starts_with(b"\x7fELF"));
        std::fs::remove_file(path).expect("remove HIR object");
    }

    #[test]
    fn emits_native_object_without_a_c_compiler() {
        let path = std::env::temp_dir().join(format!(
            "hacklang-cranelift-probe-{}.o",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        emit_probe_object(&path).expect("Cranelift object emission");
        let bytes = std::fs::read(&path).expect("read emitted object");
        assert!(!bytes.is_empty(), "object file must not be empty");

        #[cfg(target_os = "linux")]
        assert!(bytes.starts_with(b"\x7fELF"), "expected an ELF object");

        std::fs::remove_file(path).expect("remove emitted object");
    }
}
