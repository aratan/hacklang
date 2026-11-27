//! Isolated AOT object-emission spike. This module deliberately does not
//! participate in `hl build` yet: it proves Cranelift can emit a native
//! object from Rust without generating C or invoking a system C compiler.

use std::path::Path;

use cranelift_codegen::ir::{types, AbiParam};
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
        builder.finalize();
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

#[cfg(test)]
mod tests {
    use super::emit_probe_object;

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
