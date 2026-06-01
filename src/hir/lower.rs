use std::collections::{HashMap, HashSet};

use crate::{
    ast,
    hir::{
        TypeVar, TypeVarId, UnaryOperator,
        coerce::{coerce_int_literal, coerce_through_tails},
        error::{Error, ErrorKind},
        generics::{collect_typevars, ty_has_typevars, unify},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, FunctionCall,
            IntrinsicKind, LocalId, LocalIdGen, Param, Program, Statement, StatementKind,
            StructDef, Ty,
        },
    },
    lexer::types::Span,
};

#[derive(Clone)]
struct PendingFnSig {
    scope: HashMap<String, TypeVarId>,
    /// Type-var ids in declaration order. Mirrors what the eventual
    /// `GenericTemplate.type_var_ids` will hold once the body is lowered.
    type_var_ids: Vec<TypeVarId>,
    param_tys: Vec<Ty>,
    return_ty: Ty,
}

#[derive(Clone)]
pub(crate) struct GenericTemplate {
    pub name: String,
    pub span: Span,
    pub type_var_ids: Vec<TypeVarId>,
    pub params: Vec<Param>,
    pub return_ty: Ty,
    pub body: Block,
}

pub struct Lower {
    scopes: Vec<HashMap<String, LocalId>>,
    bindings: HashMap<LocalId, Ty>,
    structs: HashMap<String, StructDef>,
    id_gen: LocalIdGen,

    type_vars: Vec<TypeVar>,
    type_var_scopes: Vec<HashMap<String, TypeVarId>>,
    pending_fn_sigs: HashMap<LocalId, PendingFnSig>,
    /// Declaration-order type-var ids per function, captured during
    /// pre-registration. Outlives `pending_fn_sigs` (which is consumed when
    /// the body lowers) so `register_if_generic_template` can still see the
    /// original arity for `<$T, $U>` even when the signature mentions only
    /// some of them — the rest may still appear in the body.
    fn_decl_type_var_ids: HashMap<LocalId, Vec<TypeVarId>>,
    /// Stack of "the return type of the function whose body we're lowering
    /// right now". `return e;` reads the top to push it as a hint to `e`,
    /// so `return None;` infers from the surrounding function's return type.
    return_ty_stack: Vec<Ty>,

    pub(crate) templates: HashMap<LocalId, GenericTemplate>,

    // Cache: (template id, concrete arg types in TypeVarId order) →
    // specialization LocalId. The id is reserved BEFORE the specialization's
    // body is processed, so recursive calls find the in-progress entry.
    specialization_cache: HashMap<(LocalId, Vec<Ty>), LocalId>,
    pending_specializations: Vec<Declaration>,

    struct_templates: HashMap<String, StructDef>,
    struct_specialization_cache: HashMap<(String, Vec<Ty>), String>,
    struct_template_origin: HashMap<String, (String, Vec<Ty>)>,

    // Mirror the struct registries above.
    enums: HashMap<String, crate::hir::types::EnumDef>,
    enum_templates: HashMap<String, crate::hir::types::EnumDef>,
    enum_specialization_cache: HashMap<(String, Vec<Ty>), String>,
    enum_template_origin: HashMap<String, (String, Vec<Ty>)>,
    /// Variant name → (template enum, tag index). Names are globally unique.
    variant_constructors: HashMap<String, (String, usize)>,

    /// Ids of generic templates that came from `#comptime` declarations. When
    /// one is specialized, the substitution pass folds comptime constructs
    /// (type comparisons, `static if`) and drops unreachable code.
    comptime_template_ids: HashSet<LocalId>,
    /// True while lowering a `#comptime` function body — enables lowering of
    /// `TypeValue` / `comperror` constructs that are illegal elsewhere.
    in_comptime: bool,
    /// True while substituting a comptime template's body — enables folding.
    in_comptime_subst: bool,

    /// Top-level function names → every declaration sharing that name (an
    /// overload set, resolved by argument types at the call site).
    overloads: HashMap<String, Vec<LocalId>>,
    /// AST declaration node id → its pre-registered LocalId, so a function body
    /// finds its own id even when the name is overloaded.
    fn_id_by_node: HashMap<crate::ast::NodeId, LocalId>,
}

impl Lower {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::default()], // Add the global scope
            bindings: HashMap::default(),
            structs: HashMap::default(),
            id_gen: LocalIdGen::new(),
            type_vars: Vec::default(),
            type_var_scopes: Vec::default(),
            pending_fn_sigs: HashMap::default(),
            fn_decl_type_var_ids: HashMap::default(),
            return_ty_stack: Vec::default(),
            templates: HashMap::default(),
            specialization_cache: HashMap::default(),
            pending_specializations: Vec::default(),
            struct_templates: HashMap::default(),
            struct_specialization_cache: HashMap::default(),
            struct_template_origin: HashMap::default(),
            enums: HashMap::default(),
            enum_templates: HashMap::default(),
            enum_specialization_cache: HashMap::default(),
            enum_template_origin: HashMap::default(),
            variant_constructors: HashMap::default(),
            comptime_template_ids: HashSet::default(),
            in_comptime: false,
            in_comptime_subst: false,
            overloads: HashMap::default(),
            fn_id_by_node: HashMap::default(),
        }
    }

    fn push_type_var_scope(&mut self) {
        self.type_var_scopes.push(HashMap::new());
    }
    fn pop_type_var_scope(&mut self) -> HashMap<String, TypeVarId> {
        self.type_var_scopes
            .pop()
            .expect("type_var_scopes underflow")
    }
    fn lookup_type_var(&self, name: &str) -> Option<TypeVarId> {
        for s in self.type_var_scopes.iter().rev() {
            if let Some(&id) = s.get(name) {
                return Some(id);
            }
        }
        None
    }
    fn fresh_type_var_id(&mut self, name: String) -> TypeVarId {
        let id = TypeVarId(self.type_vars.len() as u32);
        self.type_vars.push(TypeVar { name });
        id
    }

    /// Declares each name from a `<$K, $V>` clause as a fresh type variable in
    /// the current type-var scope, so later bare references (`K`, `V`) resolve.
    /// A type-var scope must already be pushed.
    fn declare_type_params(&mut self, names: &[String], span: Span) -> Result<(), Error> {
        for name in names {
            if self.structs.contains_key(name) || self.struct_templates.contains_key(name) {
                return Err(Error {
                    span,
                    kind: ErrorKind::GenericIsAlsoAStruct { name: name.clone() },
                });
            }
            if self
                .type_var_scopes
                .last()
                .expect("declare_type_params requires a pushed scope")
                .contains_key(name)
            {
                return Err(Error {
                    span,
                    kind: ErrorKind::GenericAlreadyDefined { name: name.clone() },
                });
            }
            let id = self.fresh_type_var_id(name.clone());
            self.type_var_scopes
                .last_mut()
                .unwrap()
                .insert(name.clone(), id);
        }
        Ok(())
    }
    pub fn lower_program(&mut self, p: ast::Program) -> Result<Program, Error> {
        let span = p
            .declarations
            .first()
            .unwrap()
            .span
            .join(p.declarations.last().unwrap().span);

        // Pass 1: register every struct and enum name with an empty body so
        // bodies in pass 2 can reference each other (and themselves) freely.
        for decl in &p.declarations {
            if let Some(value) = &decl.value
                && let ast::ExprKind::EnumDef {
                    type_params,
                    variants,
                } = &value.kind
            {
                if self.enums.contains_key(&decl.name)
                    || self.enum_templates.contains_key(&decl.name)
                    || self.structs.contains_key(&decl.name)
                    || self.struct_templates.contains_key(&decl.name)
                {
                    return Err(Error {
                        span: decl.span,
                        kind: ErrorKind::AlreadyDefined {
                            name: decl.name.clone(),
                        },
                    });
                }
                for (idx, v) in variants.iter().enumerate() {
                    if let Some((other, _)) = self.variant_constructors.get(&v.name) {
                        return Err(Error {
                            span: v.span,
                            kind: ErrorKind::DuplicateVariant {
                                name: v.name.clone(),
                                other_enum: other.clone(),
                            },
                        });
                    }
                    self.variant_constructors
                        .insert(v.name.clone(), (decl.name.clone(), idx));
                }
                // TypeVarIds are pre-allocated so cross-type references in
                // pass 2 see the right arity even before bodies are lowered.
                let tv_ids: Vec<TypeVarId> = type_params
                    .iter()
                    .map(|n| self.fresh_type_var_id(n.clone()))
                    .collect();
                let placeholder = crate::hir::types::EnumDef {
                    name: decl.name.clone(),
                    type_var_ids: tv_ids,
                    variants: variants
                        .iter()
                        .map(|v| crate::hir::types::EnumVariant {
                            name: v.name.clone(),
                            fields: Vec::new(),
                        })
                        .collect(),
                };
                if type_params.is_empty() {
                    self.enums.insert(decl.name.clone(), placeholder);
                } else {
                    self.enum_templates.insert(decl.name.clone(), placeholder);
                }
                continue;
            }
            if let Some(value) = &decl.value
                && let ast::ExprKind::StructDef { type_params, .. } = &value.kind
            {
                if self.structs.contains_key(&decl.name)
                    || self.struct_templates.contains_key(&decl.name)
                    || self.enums.contains_key(&decl.name)
                    || self.enum_templates.contains_key(&decl.name)
                {
                    return Err(Error {
                        span: decl.span,
                        kind: ErrorKind::AlreadyDefined {
                            name: decl.name.clone(),
                        },
                    });
                }
                let tv_ids: Vec<TypeVarId> = type_params
                    .iter()
                    .map(|n| self.fresh_type_var_id(n.clone()))
                    .collect();
                if type_params.is_empty() {
                    self.structs.insert(
                        decl.name.clone(),
                        StructDef {
                            name: decl.name.clone(),
                            type_var_ids: tv_ids,
                            fields: Vec::new(),
                        },
                    );
                } else {
                    self.struct_templates.insert(
                        decl.name.clone(),
                        StructDef {
                            name: decl.name.clone(),
                            type_var_ids: tv_ids,
                            fields: Vec::new(),
                        },
                    );
                }
            }
        }

        // Pass 2: lower struct bodies. For generic structs, push a fresh
        // type-var scope, register each type param as a TypeVarId, then
        // lower fields. Field types may contain TypeVars and stay as
        // templates until a concrete use site triggers specialization.
        for decl in &p.declarations {
            if let Some(value) = &decl.value
                && let ast::ExprKind::StructDef {
                    type_params,
                    fields,
                } = &value.kind
            {
                if type_params.is_empty() {
                    let mut lowered_fields = Vec::with_capacity(fields.len());
                    for f in fields {
                        let ty = self.lower_type(&f.ty)?;
                        if let Ty::Struct(other) = &ty
                            && other == &decl.name
                        {
                            return Err(Error {
                                span: f.span,
                                kind: ErrorKind::DirectStructRecursion {
                                    name: decl.name.clone(),
                                },
                            });
                        }
                        lowered_fields.push((f.name.clone(), ty));
                    }
                    self.structs
                        .get_mut(&decl.name)
                        .expect("registered in pass 1")
                        .fields = lowered_fields;
                } else {
                    // Re-establish the pass-1 type-var scope.
                    let tv_ids = self
                        .struct_templates
                        .get(&decl.name)
                        .expect("registered in pass 1")
                        .type_var_ids
                        .clone();
                    self.push_type_var_scope();
                    for (name, id) in type_params.iter().zip(tv_ids.iter()) {
                        self.type_var_scopes
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), *id);
                    }

                    let mut lowered_fields = Vec::with_capacity(fields.len());
                    for f in fields {
                        let ty = self.lower_type(&f.ty)?;
                        if let Ty::Struct(other) = &ty
                            && other == &decl.name
                        {
                            self.pop_type_var_scope();
                            return Err(Error {
                                span: f.span,
                                kind: ErrorKind::DirectStructRecursion {
                                    name: decl.name.clone(),
                                },
                            });
                        }
                        lowered_fields.push((f.name.clone(), ty));
                    }
                    self.pop_type_var_scope();

                    self.struct_templates
                        .get_mut(&decl.name)
                        .expect("registered in pass 1")
                        .fields = lowered_fields;
                }
            }
        }

        // Pass 2b: lower enum variant payload types (mirrors struct bodies).
        for decl in &p.declarations {
            if let Some(value) = &decl.value
                && let ast::ExprKind::EnumDef {
                    type_params,
                    variants,
                } = &value.kind
            {
                if type_params.is_empty() {
                    let mut lowered_variants = Vec::with_capacity(variants.len());
                    for v in variants {
                        let fields: Vec<Ty> = v
                            .fields
                            .iter()
                            .map(|t| self.lower_type(t))
                            .collect::<Result<_, _>>()?;
                        lowered_variants.push(crate::hir::types::EnumVariant {
                            name: v.name.clone(),
                            fields,
                        });
                    }
                    self.enums
                        .get_mut(&decl.name)
                        .expect("registered in pass 2b")
                        .variants = lowered_variants;
                } else {
                    let tv_ids = self
                        .enum_templates
                        .get(&decl.name)
                        .expect("registered in pass 1")
                        .type_var_ids
                        .clone();
                    self.push_type_var_scope();
                    for (name, id) in type_params.iter().zip(tv_ids.iter()) {
                        self.type_var_scopes
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), *id);
                    }

                    let mut lowered_variants = Vec::with_capacity(variants.len());
                    for v in variants {
                        let fields: Vec<Ty> = v
                            .fields
                            .iter()
                            .map(|t| self.lower_type(t))
                            .collect::<Result<_, _>>()?;
                        lowered_variants.push(crate::hir::types::EnumVariant {
                            name: v.name.clone(),
                            fields,
                        });
                    }
                    self.pop_type_var_scope();

                    self.enum_templates
                        .get_mut(&decl.name)
                        .expect("registered in pass 1")
                        .variants = lowered_variants;
                }
            }
        }

        // Pass 3: function signatures.
        for decl in &p.declarations {
            self.pre_register_top_level(decl)?;
        }

        // Pass 4: lower the remaining declarations. Struct defs are skipped
        // since they're already recorded in self.structs. Generic functions
        // are detected after lowering and routed into the templates registry
        // instead of being emitted as declarations.
        let mut decls = Vec::new();
        for decl in p.declarations {
            if matches!(
                decl.value.as_ref().map(|v| &v.kind),
                Some(ast::ExprKind::StructDef { .. } | ast::ExprKind::EnumDef { .. })
            ) {
                continue;
            }
            let is_comptime = decl.comptime;
            self.in_comptime = is_comptime;
            let lowered = self.lower_declaration(decl)?;
            self.in_comptime = false;
            // Top-level decls always produce a Declaration (the alias-only
            // path is reserved for nested function literals).
            let lowered = lowered.expect("top-level declarations are never aliased away");
            if self.register_if_generic_template(&lowered) {
                if is_comptime {
                    self.comptime_template_ids.insert(lowered.id);
                }
            } else {
                decls.push(lowered);
            }
        }

        // Pass 5: drain specializations produced during body lowering. Each
        // specialization may itself produce more specializations (recursive
        // case), so we loop until quiescent.
        while !self.pending_specializations.is_empty() {
            let batch = std::mem::take(&mut self.pending_specializations);
            for spec in batch {
                decls.push(spec);
            }
        }

        Ok(Program {
            span,
            declarations: decls,
            structs: std::mem::take(&mut self.structs),
            enums: std::mem::take(&mut self.enums),
        })
    }

    /// If `decl` is a function whose signature contains a TypeVar, move it
    /// out of regular declarations into the generic-templates registry.
    /// Returns true if the declaration was consumed.
    fn register_if_generic_template(&mut self, decl: &Declaration) -> bool {
        let ExprKind::Function(func) = &decl.value.kind else {
            return false;
        };
        let is_generic =
            func.params.iter().any(|p| ty_has_typevars(&p.ty)) || ty_has_typevars(&func.return_ty);
        if !is_generic {
            return false;
        }
        // Start with declaration-order type-var ids captured at pre-registration
        // (preserves `<$T, $U>` arity even when only some appear in the
        // signature). Then append any TypeVarIds the signature mentions but
        // declaration didn't — for anonymous generics like `(v: *Vec<$T>, ...)`
        // the original `type_params` list is empty, and the `$T` only shows
        // up by walking the signature.
        let mut type_var_ids = self
            .fn_decl_type_var_ids
            .get(&decl.id)
            .cloned()
            .unwrap_or_default();
        for p in &func.params {
            collect_typevars(&p.ty, &mut type_var_ids);
        }
        collect_typevars(&func.return_ty, &mut type_var_ids);
        self.templates.insert(
            decl.id,
            GenericTemplate {
                name: decl.name.clone(),
                span: decl.span,
                type_var_ids,
                params: func.params.clone(),
                return_ty: func.return_ty.clone(),
                body: func.body.clone(),
            },
        );
        true
    }

    fn pre_register_top_level(&mut self, d: &ast::Declaration) -> Result<(), Error> {
        let Some(value) = &d.value else {
            return Ok(());
        };
        let ast::ExprKind::Function {
            type_params,
            params,
            return_ty,
            ..
        } = &value.kind
        else {
            return Ok(());
        };

        // Each top-level function gets its own type-variable scope. After
        // the signature is lowered the scope is saved on the side so the
        // body's $T / T references resolve to the same TypeVarIds.
        self.push_type_var_scope();
        let lowered_sig = (|| -> Result<(Vec<Ty>, Ty), Error> {
            self.declare_type_params(type_params, d.span)?;
            let param_tys = params
                .iter()
                .map(|p| self.lower_type(&p.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = match return_ty {
                Some(t) => self.lower_type(t)?,
                None => Ty::Unit,
            };
            Ok((param_tys, ret))
        })();
        let scope = self.pop_type_var_scope();
        let (param_tys, return_ty) = lowered_sig?;
        // Save type-var ids in declaration order so a later TypedFunctionRef
        // can know the template's arity before its body has been lowered.
        let type_var_ids: Vec<TypeVarId> = type_params
            .iter()
            .filter_map(|n| scope.get(n).copied())
            .collect();

        let ty = Ty::Function {
            params: param_tys.clone(),
            return_ty: Box::new(return_ty.clone()),
            varargs: false,
        };

        let id = self.id_gen.fresh();
        // Functions may share a name (overloading); the scope keeps the most
        // recent for bare references, while `overloads` keeps the full set and
        // `fn_id_by_node` ties each declaration to its own id.
        self.current_scope_mut().insert(d.name.clone(), id);
        self.overloads.entry(d.name.clone()).or_default().push(id);
        self.fn_id_by_node.insert(d.id, id);
        self.bindings.insert(id, ty);
        self.fn_decl_type_var_ids.insert(id, type_var_ids.clone());
        self.pending_fn_sigs.insert(
            id,
            PendingFnSig {
                scope,
                type_var_ids,
                param_tys,
                return_ty,
            },
        );

        Ok(())
    }

    fn lower_declaration(&mut self, d: ast::Declaration) -> Result<Option<Declaration>, Error> {
        // Top-level functions were pre-registered (by node id, so overloaded
        // names still find the right id). Everything else is a fresh binding.
        let pre_registered_id = if matches!(
            d.value.as_ref().map(|v| &v.kind),
            Some(ast::ExprKind::Function { .. })
        ) {
            self.fn_id_by_node.get(&d.id).copied()
        } else {
            None
        };

        if pre_registered_id.is_none() && self.current_scope().contains_key(&d.name) {
            return Err(Error {
                span: d.span,
                kind: ErrorKind::AlreadyDefined { name: d.name },
            });
        }

        // `let name = extern (...);` — only allowed at top level. Builds a
        // function-typed binding whose value is an ExternFunction marker;
        // codegen registers the corresponding LLVM symbol with External
        // linkage and no body.
        if let Some(value_ast) = &d.value
            && matches!(value_ast.kind, ast::ExprKind::ExternFunction { .. })
        {
            if self.scopes.len() > 1 {
                return Err(Error {
                    span: d.span,
                    kind: ErrorKind::ExternMustBeTopLevel,
                });
            }
            let value_ast = d.value.unwrap();
            let span = value_ast.span;
            let ast::ExprKind::ExternFunction {
                c_name,
                params: ast_params,
                varargs,
                return_ty,
            } = value_ast.kind
            else {
                unreachable!();
            };
            let mut hir_params = Vec::with_capacity(ast_params.len());
            for p in ast_params {
                let ty = self.lower_type(&p.ty)?;
                let id = self.id_gen.fresh();
                hir_params.push(Param {
                    id,
                    span: p.span,
                    name: p.name,
                    ty,
                });
            }
            let ret_ty = match return_ty {
                Some(t) => self.lower_type(&t)?,
                None => Ty::Unit,
            };
            let fn_ty = Ty::Function {
                params: hir_params.iter().map(|p| p.ty.clone()).collect(),
                return_ty: Box::new(ret_ty.clone()),
                varargs,
            };
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(d.name.clone(), id);
            self.bindings.insert(id, fn_ty.clone());
            self.overloads.entry(d.name.clone()).or_default().push(id);
            let c_name = c_name.unwrap_or_else(|| d.name.clone());
            return Ok(Some(Declaration {
                id,
                span: d.span,
                name: d.name,
                ty: fn_ty.clone(),
                value: Expr {
                    span,
                    ty: fn_ty,
                    kind: ExprKind::ExternFunction {
                        c_name,
                        params: hir_params,
                        return_ty: ret_ty,
                        varargs,
                    },
                },
            }));
        }

        // Pre-registered top-level functions: lower the body using the
        // already-lowered signature (don't re-call lower_type on `$T`).
        if let Some(id) = pre_registered_id {
            if d.ty.is_some() {
                return Err(Error {
                    span: d.span,
                    kind: ErrorKind::TypeAnnotationNotAllowed,
                });
            }
            let sig = self
                .pending_fn_sigs
                .remove(&id)
                .expect("pre_register_top_level must save a sig for every function");

            let value_ast = d.value.expect("pre_registered guarantees a Function value");
            let ast::ExprKind::Function { params, body, .. } = value_ast.kind else {
                unreachable!("pre_registered guarantees Function");
            };

            let value = self.lower_top_level_function_body(params, body, sig, value_ast.span)?;
            let ty = value.ty.clone();
            return Ok(Some(Declaration {
                id,
                span: d.span,
                name: d.name,
                ty,
                value,
            }));
        }

        // Annotation is lowered first, then pushed as a hint into the value.
        let expected = match &d.ty {
            Some(t) => Some(self.lower_type(t)?),
            None => None,
        };

        // `let x: T;` — no initializer. Synthesize a zero-pattern value of T.
        let Some(value_ast) = d.value else {
            let target = expected.ok_or_else(|| Error {
                span: d.span,
                kind: ErrorKind::MissingTypeForZeroInit,
            })?;
            if ty_has_typevars(&target) {
                return Err(Error {
                    span: d.span,
                    kind: ErrorKind::ZeroInitOfGenericType,
                });
            }
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(d.name.clone(), id);
            self.bindings.insert(id, target.clone());
            return Ok(Some(Declaration {
                id,
                span: d.span,
                name: d.name,
                ty: target.clone(),
                value: Expr {
                    span: d.span,
                    ty: target.clone(),
                    kind: ExprKind::ZeroInit(target),
                },
            }));
        };

        let mut value = self.lower_expr_with_hint(value_ast, expected.as_ref())?;
        if let Some(target) = &expected {
            value = coerce_through_tails(value, target)?;
            if value.ty != *target {
                return Err(Error {
                    span: value.span,
                    kind: ErrorKind::TypeMismatch {
                        expected: target.clone(),
                        found: value.ty.clone(),
                    },
                });
            }
        }

        // Nested function literal: the value is `Local(synth_id)` referencing
        // a lifted top-level (template or monomorphic). Alias the user name
        // directly to the synth id so call sites resolve to the same id the
        // template / function table knows — no runtime indirection, and
        // generic templates remain dispatchable.
        if let ExprKind::Local(synth_id) = value.kind
            && matches!(value.ty, Ty::Function { .. })
        {
            self.current_scope_mut().insert(d.name, synth_id);
            return Ok(None);
        }

        let ty = value.ty.clone();
        let id = self.id_gen.fresh();
        self.current_scope_mut().insert(d.name.clone(), id);
        self.bindings.insert(id, ty.clone());

        Ok(Some(Declaration {
            id,
            span: d.span,
            name: d.name,
            ty,
            value,
        }))
    }

    fn lower_top_level_function_body(
        &mut self,
        params: Vec<ast::Param>,
        body: ast::Block,
        sig: PendingFnSig,
        span: crate::lexer::types::Span,
    ) -> Result<Expr, Error> {
        // Re-push the type-var scope so any `T` references inside the body
        // resolve to the TypeVarIds chosen during pre-register.
        self.type_var_scopes.push(sig.scope);

        self.enter_scope();
        let mut hir_params = Vec::with_capacity(params.len());
        for (p, ty) in params.into_iter().zip(sig.param_tys.iter()) {
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(p.name.clone(), id);
            self.bindings.insert(id, ty.clone());
            hir_params.push(Param {
                id,
                span: p.span,
                name: p.name,
                ty: ty.clone(),
            });
        }

        // Return type flows into the body's tail AND into any `return e;`.
        // TypeVar-bearing hints are still useful for variant constructors —
        // they record the deferred specialization until the call site picks K/V.
        self.return_ty_stack.push(sig.return_ty.clone());
        let body_result = self.lower_block_with_hint(body, Some(&sig.return_ty));
        self.return_ty_stack.pop();
        self.leave_scope();
        self.pop_type_var_scope();

        let mut body = body_result?;
        if body.tail.ty != sig.return_ty && sig.return_ty.is_number() && sig.return_ty != Ty::Int {
            let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
            body.tail = Box::new(coerce_through_tails(*tail, &sig.return_ty)?);
        }

        let fn_ty = Ty::Function {
            params: hir_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(sig.return_ty.clone()),
            varargs: false,
        };
        Ok(Expr {
            span,
            ty: fn_ty,
            kind: ExprKind::Function(Function {
                params: hir_params,
                return_ty: sig.return_ty,
                body,
            }),
        })
    }
    fn lower_expr(&mut self, e: ast::Expr) -> Result<Expr, Error> {
        self.lower_expr_with_hint(e, None)
    }

    /// `hint` is the surrounding expected type, used today only as a tiebreaker
    /// for nullary variants like `None` where neither explicit type args nor
    /// argument unification can pin the enum's type parameters.
    fn lower_expr_with_hint(
        &mut self,
        e: ast::Expr,
        hint: Option<&Ty>,
    ) -> Result<Expr, Error> {
        match e.kind {
            ast::ExprKind::Const(ast::Const::Int(n)) => Ok(Expr {
                span: e.span,
                ty: Ty::Int,
                kind: ExprKind::Const(Const::Int(n)),
            }),
            ast::ExprKind::Const(ast::Const::Float(f)) => Ok(Expr {
                span: e.span,
                ty: Ty::Float,
                kind: ExprKind::Const(Const::Float(f)),
            }),
            ast::ExprKind::Const(ast::Const::Str(s)) => Ok(Expr {
                span: e.span,
                ty: Ty::Ptr(Box::new(Ty::U8)),
                kind: ExprKind::Const(Const::Str(s)),
            }),
            ast::ExprKind::Const(ast::Const::Char(b)) => Ok(Expr {
                span: e.span,
                ty: Ty::U8,
                kind: ExprKind::Const(Const::Char(b)),
            }),
            ast::ExprKind::Identifier(name) => {
                // Inside a `#comptime` body, a bare type-variable name used in
                // expression position (e.g. `T` in `T == Int`) denotes a
                // reified type value rather than a runtime binding.
                if self.in_comptime
                    && self.resolve(&name).is_none()
                    && let Some(tv) = self.lookup_type_var(&name)
                {
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::TypeValue(Ty::TypeVar(tv)),
                    });
                }

                // Variant names shadow locals — `None` is reserved.
                if self.variant_constructors.contains_key(&name) {
                    if let Some(expr) =
                        self.try_lower_variant_call(&name, &[], Vec::new(), e.span, hint)?
                    {
                        return Ok(expr);
                    }
                }

                let Some(local_id) = self.resolve(&name) else {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::NameNotFound { name },
                    });
                };

                let ty = self
                    .bindings
                    .get(&local_id)
                    .expect("resolved name must have a recorded type")
                    .clone();

                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Local(local_id),
                })
            }
            ast::ExprKind::TypeValue(type_expr) => {
                if !self.in_comptime {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::ComptimeError {
                            message:
                                "a type can only be used as a value inside a #comptime function"
                                    .to_string(),
                        },
                    });
                }
                let ty = self.lower_type(&type_expr)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::TypeValue(ty),
                })
            }
            ast::ExprKind::Function {
                type_params,
                params,
                return_ty,
                body,
            } => {
                // Inline (named or anonymous) function literals: get their
                // own type-var scope. `lower_function_value` lifts the result
                // to a synthesized top-level function and returns a Local ref.
                self.push_type_var_scope();
                let result = self
                    .declare_type_params(&type_params, e.span)
                    .and_then(|()| self.lower_function_value(params, return_ty, body, e.span));
                self.pop_type_var_scope();
                result
            }
            ast::ExprKind::Closure { params, body } => {
                self.lower_closure(params, *body, hint, e.span)
            }
            ast::ExprKind::ExternFunction { .. } => Err(Error {
                span: e.span,
                kind: ErrorKind::ExternMustBeTopLevel,
            }),
            ast::ExprKind::Null => {
                let ty = match hint {
                    Some(t @ Ty::Ptr(_)) => t.clone(),
                    _ => {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::CannotInferNullType,
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty: ty.clone(),
                    kind: ExprKind::ZeroInit(ty),
                })
            }
            ast::ExprKind::TypedFunctionRef { name, type_args } => {
                let template_id = self.resolve(&name).ok_or_else(|| Error {
                    span: e.span,
                    kind: ErrorKind::NameNotFound { name: name.clone() },
                })?;
                let lowered_args: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.lower_type(t))
                    .collect::<Result<_, _>>()?;

                // The template may already be fully lowered (in self.templates)
                // or may only have its signature registered (in pending_fn_sigs)
                // if its body hasn't been lowered yet. Either source gives us
                // the arity + signature info we need.
                let (template_name, type_var_ids, param_tys, return_ty) =
                    if let Some(t) = self.templates.get(&template_id) {
                        (
                            t.name.clone(),
                            t.type_var_ids.clone(),
                            t.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
                            t.return_ty.clone(),
                        )
                    } else if let Some(sig) = self.pending_fn_sigs.get(&template_id) {
                        (
                            name.clone(),
                            sig.type_var_ids.clone(),
                            sig.param_tys.clone(),
                            sig.return_ty.clone(),
                        )
                    } else {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::NotAGenericFunction { name },
                        });
                    };
                if lowered_args.len() != type_var_ids.len() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::TypeArgArityMismatch {
                            name: template_name,
                            expected: type_var_ids.len(),
                            found: lowered_args.len(),
                        },
                    });
                }
                // Build the function type with the type args substituted in,
                // so the resulting expression's type is correct even in the
                // deferred case.
                let mut subst = HashMap::new();
                for (tv, t) in type_var_ids.iter().zip(lowered_args.iter()) {
                    subst.insert(*tv, t.clone());
                }
                let result_param_tys: Vec<Ty> =
                    param_tys.iter().map(|p| self.substitute_ty(p, &subst)).collect();
                let result_return_ty = self.substitute_ty(&return_ty, &subst);
                let fn_ty = Ty::Function {
                    params: result_param_tys,
                    return_ty: Box::new(result_return_ty),
                    varargs: false,
                };
                // Specialize now only when both (a) the args are concrete and
                // (b) the template's body is available. Otherwise emit a
                // DeferredFunctionRef the substitution pass will resolve.
                let deferable = lowered_args.iter().any(ty_has_typevars)
                    || !self.templates.contains_key(&template_id);
                if deferable {
                    return Ok(Expr {
                        span: e.span,
                        ty: fn_ty,
                        kind: ExprKind::DeferredFunctionRef {
                            template_id,
                            type_args: lowered_args,
                        },
                    });
                }
                let specialized =
                    self.specialize_call(template_id, &[], &lowered_args, e.span)?;
                let final_ty = self.bindings[&specialized].clone();
                Ok(Expr {
                    span: e.span,
                    ty: final_ty,
                    kind: ExprKind::Local(specialized),
                })
            }
            ast::ExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                // `comperror("msg")` inside a comptime body lowers to a
                // CompError node that aborts compilation if reached during
                // comptime evaluation.
                if self.in_comptime
                    && let ast::ExprKind::Identifier(n) = &callee.kind
                    && n == "comperror"
                {
                    let message = match args.first().map(|a| &a.kind) {
                        Some(ast::ExprKind::Const(ast::Const::Str(s))) => s.clone(),
                        _ => {
                            return Err(Error {
                                span: e.span,
                                kind: ErrorKind::ComptimeError {
                                    message: "comperror expects a single string-literal argument"
                                        .to_string(),
                                },
                            });
                        }
                    };
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::CompError(message),
                    });
                }

                // Heap intrinsics: `alloc<T>(n)`, `realloc<T>(p, n)`, `free(p)`.
                if let ast::ExprKind::Identifier(n) = &callee.kind {
                    let intrinsic = match n.as_str() {
                        "alloc" => Some(IntrinsicKind::Alloc),
                        "realloc" => Some(IntrinsicKind::Realloc),
                        "free" => Some(IntrinsicKind::Free),
                        _ => None,
                    };
                    if let Some(kind) = intrinsic {
                        return self.lower_intrinsic(kind, type_args, args, e.span);
                    }
                }

                // Enum variant constructor: `Some(x)`, `Ok<Int, String>(42)`.
                if let ast::ExprKind::Identifier(n) = &callee.kind
                    && self.variant_constructors.contains_key(n)
                {
                    let name = n.clone();
                    if let Some(expr) =
                        self.try_lower_variant_call(&name, &type_args, args, e.span, hint)?
                    {
                        return Ok(expr);
                    }
                    unreachable!("variant_constructors entry guarantees Ok(Some)");
                }

                // Overloaded direct call: pick the overload matching the args.
                if let ast::ExprKind::Identifier(name) = &callee.kind
                    && self.overloads.get(name).is_some_and(|c| c.len() > 1)
                {
                    let name = name.clone();
                    let has_closures = args
                        .iter()
                        .any(|a| matches!(a.kind, ast::ExprKind::Closure { .. }));
                    let lowered_args: Vec<Expr> = if has_closures {
                        self.lower_overloaded_call_args_with_closures(&name, args)?
                    } else {
                        args.into_iter()
                            .map(|a| self.lower_expr(a))
                            .collect::<Result<_, _>>()?
                    };
                    let arg_tys: Vec<Ty> = lowered_args.iter().map(|a| a.ty.clone()).collect();
                    let explicit_type_args: Vec<Ty> = type_args
                        .iter()
                        .map(|t| self.lower_type(t))
                        .collect::<Result<_, _>>()?;
                    let fn_id = self.resolve_overload(&name, &arg_tys, e.span)?;
                    let callee = Expr {
                        span: e.span,
                        ty: self.bindings[&fn_id].clone(),
                        kind: ExprKind::Local(fn_id),
                    };
                    return self.finish_call(callee, lowered_args, explicit_type_args, e.span);
                }

                // Method call / UFCS: `recv.name(args)` becomes a field call
                // (when `name` is a field holding a function) or the uniform
                // form `name(recv, args)` otherwise.
                if matches!(&callee.kind, ast::ExprKind::Field { .. }) {
                    return self.lower_method_call(*callee, type_args, args, e.span);
                }

                let callee = self.lower_expr(*callee)?;
                let explicit_type_args: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.lower_type(t))
                    .collect::<Result<_, _>>()?;
                let args = self.lower_call_args(&callee, args, &explicit_type_args, e.span)?;
                self.finish_call(callee, args, explicit_type_args, e.span)
            }
            ast::ExprKind::Unary { op, expr } => {
                let operand = self.lower_expr(*expr)?;
                let op = self.lower_unary(op);
                // `!x` always yields an Int (0/1). `-x` preserves the operand type.
                let ty = match op {
                    UnaryOperator::Not => Ty::Int,
                    UnaryOperator::Minus => operand.ty.clone(),
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                })
            }
            ast::ExprKind::Binary { op, lhs, rhs } => {
                use crate::ast::BinaryOperator as B;
                // For `==`/`!=`, lower LHS first; if it's a pointer, push its
                // type to the RHS as a hint so `null` picks up the right
                // pointer type.
                let lhs = self.lower_expr(*lhs)?;
                let rhs_hint =
                    matches!(op, B::Eq | B::Ne).then(|| lhs.ty.clone());
                let rhs = self.lower_expr_with_hint(*rhs, rhs_hint.as_ref())?;
                let (lhs, rhs) = if lhs.ty != rhs.ty {
                    if rhs.ty.is_number() && rhs.ty != Ty::Int {
                        let target = rhs.ty.clone();
                        (coerce_int_literal(lhs, &target)?, rhs)
                    } else if lhs.ty.is_number() && lhs.ty != Ty::Int {
                        let target = lhs.ty.clone();
                        (lhs, coerce_int_literal(rhs, &target)?)
                    } else {
                        (lhs, rhs)
                    }
                } else {
                    (lhs, rhs)
                };
                let ty = match op {
                    // Arithmetic preserves the operand type (operands must match —
                    // typechecker enforces this).
                    B::Add | B::Sub | B::Mul | B::Div | B::Mod => lhs.ty.clone(),
                    // Shifts and bitwise are Int-only; result is Int.
                    B::Shl | B::Shr | B::BitAnd | B::BitOr | B::BitXor => Ty::Int,
                    // Comparisons and logical produce Int (0/1) as boolean.
                    B::Lt | B::Le | B::Gt | B::Ge | B::Eq | B::Ne => Ty::Int,
                    B::And | B::Or => Ty::Int,
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                })
            }

            ast::ExprKind::Block(block) => {
                let b = self.lower_block_with_hint(block, hint)?;
                let ty = b.tail.ty.clone();
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Block(b),
                })
            }
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_expr(*condition)?;

                let then_block = self.lower_block_with_hint(then_branch, hint)?;
                let then_span = then_block.span;
                let then_expr = Expr {
                    span: then_span,
                    ty: then_block.tail.ty.clone(),
                    kind: ExprKind::Block(then_block),
                };

                let else_expr = match else_branch {
                    Some(else_branch) => self.lower_expr_with_hint(*else_branch, hint)?,
                    None => unit_expr(end_of(e.span)),
                };

                let (then_expr, else_expr) = if then_expr.ty != else_expr.ty {
                    if else_expr.ty.is_number() && else_expr.ty != Ty::Int {
                        let target = else_expr.ty.clone();
                        (coerce_through_tails(then_expr, &target)?, else_expr)
                    } else if then_expr.ty.is_number() && then_expr.ty != Ty::Int {
                        let target = then_expr.ty.clone();
                        (then_expr, coerce_through_tails(else_expr, &target)?)
                    } else {
                        (then_expr, else_expr)
                    }
                } else {
                    (then_expr, else_expr)
                };

                let if_ty = then_expr.ty.clone();
                Ok(Expr {
                    span: e.span,
                    ty: if_ty,
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(then_expr),
                        else_branch: Box::new(else_expr),
                    },
                })
            }
            ast::ExprKind::Cast { expr, target } => {
                let ty = self.lower_type(&target)?;
                Ok(Expr {
                    span: e.span,
                    ty: ty.clone(),
                    kind: ExprKind::Cast {
                        target: ty.clone(),
                        expr: Box::new(self.lower_expr(*expr)?),
                    },
                })
            }
            ast::ExprKind::Assign { target, value } => {
                // The parser guarantees `target` is a place expression
                // (Identifier or Subscript). Lower it like any other expr
                // — Identifier becomes Local, Subscript becomes Subscript.
                let target = self.lower_expr(*target)?;
                // Push the target's type as a hint into the value, so
                // `x = Empty;` (etc.) can infer from the target.
                let value_hint = target.ty.clone();
                let value = self.lower_expr_with_hint(*value, Some(&value_hint))?;
                let value = coerce_int_literal(value, &target.ty)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::Assign {
                        target: Box::new(target),
                        value: Box::new(value),
                    },
                })
            }
            ast::ExprKind::Subscript { expr, index } => {
                let target = self.lower_expr(*expr)?;
                let index = self.lower_expr(*index)?;
                // Indexing works on both fixed arrays and raw pointers.
                let element = match target.ty.clone() {
                    Ty::Array { element, .. } => *element,
                    Ty::Ptr(element) => *element,
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotIndexable { found: other },
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty: element,
                    kind: ExprKind::Subscript {
                        expr: Box::new(target),
                        index: Box::new(index),
                    },
                })
            }
            ast::ExprKind::Array(items) => {
                if items.is_empty() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::EmptyArrayLiteral,
                    });
                }
                let lowered: Vec<Expr> = items
                    .into_iter()
                    .map(|x| self.lower_expr(x))
                    .collect::<Result<_, _>>()?;
                let element = Box::new(lowered[0].ty.clone());
                let count = lowered.len();
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Array { element, count },
                    kind: ExprKind::Array(lowered),
                })
            }
            ast::ExprKind::Ref(target) => {
                let target = self.lower_expr(*target)?;
                let ty = Ty::Ptr(Box::new(target.ty.clone()));
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Ref(Box::new(target)),
                })
            }
            ast::ExprKind::Deref(target) => {
                let target = self.lower_expr(*target)?;
                // `*u8` in a `#comptime` body — the unary `*` parses as Deref
                // but semantically wraps the inner type. Rewrite to a TypeValue
                // so `T == *u8` works.
                if let ExprKind::TypeValue(inner_ty) = target.kind {
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::TypeValue(Ty::Ptr(Box::new(inner_ty))),
                    });
                }
                let ty = match &target.ty {
                    Ty::Ptr(inner) => (**inner).clone(),
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotDereferencable {
                                found: other.clone(),
                            },
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Deref(Box::new(target)),
                })
            }
            ast::ExprKind::StructDef { .. } => {
                // Struct definitions at top level are consumed by lower_program
                // and never reach here. Inside an expression context they have
                // no runtime value.
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::StructDefNotAllowedHere,
                })
            }
            ast::ExprKind::StructLiteral {
                name,
                type_args,
                fields,
            } => {
                // Explicit type arguments: `Map<K, V> { ... }`.
                if !type_args.is_empty() {
                    if let Some(template) = self.struct_templates.get(&name).cloned() {
                        return self.lower_explicit_generic_struct_literal(
                            template, name, type_args, fields, e.span,
                        );
                    }
                    if self.structs.contains_key(&name) {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::UnexpectedTypeArguments { name },
                        });
                    }
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::UnknownType { name },
                    });
                }
                // Concrete-struct path: existing behavior.
                if let Some(def) = self.structs.get(&name).cloned() {
                    return self.lower_struct_literal(def, name, fields, e.span);
                }
                // Generic-struct path: infer type args from the provided field
                // values, then specialize.
                if let Some(template) = self.struct_templates.get(&name).cloned() {
                    return self.lower_generic_struct_literal(template, name, fields, e.span);
                }
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::UnknownType { name: name.clone() },
                })
            }
            ast::ExprKind::While { condition, body } => {
                let condition = self.lower_expr(*condition)?;
                let body = self.lower_block(body)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::While {
                        condition: Box::new(condition),
                        body,
                    },
                })
            }
            ast::ExprKind::Field { target, name } => {
                let mut target = self.lower_expr(*target)?;
                // Auto-deref through any chain of pointers so `p.x` works
                // when `p: *T`, `**T`, etc. If the chain doesn't end in a
                // struct, the NotAStruct check below catches it.
                while let Ty::Ptr(inner) = target.ty.clone() {
                    let pointee_ty = *inner;
                    target = Expr {
                        span: target.span,
                        ty: pointee_ty,
                        kind: ExprKind::Deref(Box::new(target)),
                    };
                }
                let (display_name, fields) = match &target.ty {
                    Ty::Struct(n) => {
                        let def = self
                            .structs
                            .get(n)
                            .expect("typed Struct must be registered");
                        (n.clone(), def.fields.clone())
                    }
                    Ty::GenericStruct { name, args } => {
                        let template = self
                            .struct_templates
                            .get(name)
                            .cloned()
                            .expect("GenericStruct must reference a known template");
                        let subst: HashMap<TypeVarId, Ty> = template
                            .type_var_ids
                            .iter()
                            .zip(args.iter())
                            .map(|(tv, ty)| (*tv, ty.clone()))
                            .collect();
                        let substituted_fields: Vec<(String, Ty)> = template
                            .fields
                            .iter()
                            .map(|(fname, fty)| {
                                let ty = self.substitute_ty(fty, &subst);
                                (fname.clone(), ty)
                            })
                            .collect();
                        (name.clone(), substituted_fields)
                    }
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotAStruct {
                                found: other.clone(),
                            },
                        });
                    }
                };

                let (index, field_ty) = fields
                    .iter()
                    .enumerate()
                    .find_map(|(i, (n, t))| (n == &name).then_some((i, t.clone())))
                    .ok_or_else(|| Error {
                        span: e.span,
                        kind: ErrorKind::UnknownField {
                            struct_name: display_name,
                            field: name.clone(),
                        },
                    })?;

                Ok(Expr {
                    span: e.span,
                    ty: field_ty,
                    kind: ExprKind::Field {
                        target: Box::new(target),
                        name,
                        index,
                    },
                })
            }
            ast::ExprKind::Tuple(elems) => {
                let lowered: Vec<Expr> = elems
                    .into_iter()
                    .map(|el| self.lower_expr(el))
                    .collect::<Result<_, _>>()?;
                let ty = Ty::Tuple(lowered.iter().map(|el| el.ty.clone()).collect());
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Tuple(lowered),
                })
            }
            ast::ExprKind::TupleField { target, index } => {
                let mut target = self.lower_expr(*target)?;
                // Auto-deref through any pointer chain so `p.0` works when
                // `p: *T`, `**T`, ...
                while let Ty::Ptr(inner) = target.ty.clone() {
                    let pointee_ty = *inner;
                    target = Expr {
                        span: target.span,
                        ty: pointee_ty,
                        kind: ExprKind::Deref(Box::new(target)),
                    };
                }
                let elems = match &target.ty {
                    Ty::Tuple(elems) => elems.clone(),
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotATuple {
                                found: other.clone(),
                            },
                        });
                    }
                };
                if index >= elems.len() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::TupleIndexOutOfRange {
                            len: elems.len(),
                            index,
                        },
                    });
                }
                let field_ty = elems[index].clone();
                Ok(Expr {
                    span: e.span,
                    ty: field_ty,
                    kind: ExprKind::TupleField {
                        target: Box::new(target),
                        index,
                    },
                })
            }
            ast::ExprKind::EnumDef { .. } => {
                // Enum definitions at top level are consumed by lower_program
                // and never reach here. Inside an expression context they have
                // no runtime value.
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::StructDefNotAllowedHere,
                })
            }
            ast::ExprKind::Match { scrutinee, arms } => {
                self.lower_match(*scrutinee, arms, e.span, hint)
            }
        }
    }

    /// Finishes a call once the callee and arguments are lowered: coerces
    /// int-literal args against the parameter types, then either specializes a
    /// generic template (when every argument is concrete) or builds the call.
    fn finish_call(
        &mut self,
        callee: Expr,
        args: Vec<Expr>,
        explicit_type_args: Vec<Ty>,
        span: Span,
    ) -> Result<Expr, Error> {
        let Ty::Function { params, return_ty, .. } = callee.ty.clone() else {
            return Err(Error {
                span: callee.span,
                kind: ErrorKind::NotCallable {
                    found: callee.ty.clone(),
                },
            });
        };

        // Coerce int literals against the (possibly generic) param type;
        // coerce_int_literal is a no-op when the target is a TypeVar.
        let mut coerced = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            let arg = match params.get(i) {
                Some(pty) => coerce_int_literal(arg, pty)?,
                None => arg,
            };
            coerced.push(arg);
        }
        let args = coerced;

        let callee_local = if let ExprKind::Local(id) = callee.kind {
            Some(id)
        } else {
            None
        };
        if let Some(callee_id) = callee_local
            && self.templates.contains_key(&callee_id)
        {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            let concrete = !arg_tys.iter().any(ty_has_typevars)
                && !explicit_type_args.iter().any(ty_has_typevars);
            if concrete {
                let new_id = self.specialize_call(callee_id, &arg_tys, &explicit_type_args, span)?;
                let new_fn_ty = self.bindings[&new_id].clone();
                let Ty::Function {
                    return_ty: spec_ret,
                    ..
                } = &new_fn_ty
                else {
                    unreachable!("specialized function must have function type");
                };
                let result_ty = (**spec_ret).clone();
                return Ok(Expr {
                    span,
                    ty: result_ty,
                    kind: ExprKind::Call(FunctionCall {
                        callee: Box::new(Expr {
                            span: callee.span,
                            ty: new_fn_ty,
                            kind: ExprKind::Local(new_id),
                        }),
                        args,
                    }),
                });
            }
        }

        // Couldn't fully specialize, but if the callee is a generic template
        // we can still unify the partial info — any TypeVars we *do* learn
        // get substituted into the return type. Without this, the call's
        // result type stays as the template's bare TypeVar(T), which means
        // `match self.vec.get(i)` sees `T` instead of the actual `Bucket<K, V>`.
        let result_ty = if let Some(callee_id) = callee_local
            && let Some(template) = self.templates.get(&callee_id).cloned()
            && template.params.len() == args.len()
        {
            let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
            if explicit_type_args.len() == template.type_var_ids.len() {
                for (tv, t) in template
                    .type_var_ids
                    .iter()
                    .zip(explicit_type_args.iter())
                {
                    subst.insert(*tv, t.clone());
                }
            }
            for (param, arg) in template.params.iter().zip(args.iter()) {
                let _ = unify(
                    &param.ty,
                    &arg.ty,
                    span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                );
            }
            self.substitute_ty(&template.return_ty, &subst)
        } else {
            *return_ty
        };
        Ok(Expr {
            span,
            ty: result_ty,
            kind: ExprKind::Call(FunctionCall {
                callee: Box::new(callee),
                args,
            }),
        })
    }

    /// Lowers `recv.name(args)`. If `name` is a (function-typed) field of the
    /// receiver's struct it's a field call; otherwise it desugars to the
    /// uniform-call form `name(recv, args)` (Nim-style UFCS), auto-referencing
    /// the receiver when the function expects a pointer.
    fn lower_method_call(
        &mut self,
        callee: ast::Expr,
        type_args: Vec<ast::TypeExpr>,
        args: Vec<ast::Expr>,
        span: Span,
    ) -> Result<Expr, Error> {
        let ast::ExprKind::Field { target, name } = callee.kind else {
            unreachable!("lower_method_call requires a field callee");
        };
        let recv = self.lower_expr(*target)?;
        let explicit_type_args: Vec<Ty> = type_args
            .iter()
            .map(|t| self.lower_type(t))
            .collect::<Result<_, _>>()?;

        // `recv.field(args)` — only when the field actually holds a function
        // value. A non-callable field (e.g. `s.len` is an `Int`) falls
        // through to UFCS instead, so `s.len()` can call a top-level `len`
        // that takes `s` as its receiver.
        if let Some((index, field_ty)) = self.resolve_field(&recv.ty, &name)
            && matches!(field_ty, Ty::Function { .. })
        {
            let target = self.autoderef_to_struct(recv);
            let callee = Expr {
                span: target.span,
                ty: field_ty,
                kind: ExprKind::Field {
                    target: Box::new(target),
                    name,
                    index,
                },
            };
            let args = self.lower_call_args(&callee, args, &explicit_type_args, span)?;
            return self.finish_call(callee, args, explicit_type_args, span);
        }

        // UFCS: resolve `name` (possibly overloaded) against the receiver and
        // arguments, then call it as `name(recv, args)`. When an arg is a
        // closure literal, route through the two-pass helper so it can pick
        // up its parameter types from the narrowed signature.
        let has_closures = args
            .iter()
            .any(|a| matches!(a.kind, ast::ExprKind::Closure { .. }));
        let lowered_args: Vec<Expr> = if has_closures {
            self.lower_method_call_args_with_closures(&name, &recv, args)?
        } else {
            args.into_iter()
                .map(|a| self.lower_expr(a))
                .collect::<Result<_, _>>()?
        };
        let other_arg_tys: Vec<Ty> = lowered_args.iter().map(|a| a.ty.clone()).collect();
        let (fn_id, recv) = self.resolve_method(&name, recv, &other_arg_tys, span)?;
        let callee = Expr {
            span,
            ty: self.bindings[&fn_id].clone(),
            kind: ExprKind::Local(fn_id),
        };
        let mut all_args = Vec::with_capacity(lowered_args.len() + 1);
        all_args.push(recv);
        all_args.extend(lowered_args);
        self.finish_call(callee, all_args, explicit_type_args, span)
    }

    /// Looks up `name` as a field of `ty` (after auto-dereferencing pointers to
    /// a struct), returning its index and substituted type. None if `ty` is not
    /// a struct or has no such field.
    fn resolve_field(&mut self, ty: &Ty, name: &str) -> Option<(usize, Ty)> {
        let mut t = ty.clone();
        while let Ty::Ptr(inner) = t {
            t = *inner;
        }
        let fields: Vec<(String, Ty)> = match t {
            Ty::Struct(n) => self.structs.get(&n)?.fields.clone(),
            Ty::GenericStruct { name: sname, args } => {
                let template = self.struct_templates.get(&sname)?.clone();
                let subst: HashMap<TypeVarId, Ty> = template
                    .type_var_ids
                    .iter()
                    .zip(args.iter())
                    .map(|(tv, ty)| (*tv, ty.clone()))
                    .collect();
                template
                    .fields
                    .iter()
                    .map(|(fname, fty)| (fname.clone(), self.substitute_ty(fty, &subst)))
                    .collect()
            }
            _ => return None,
        };
        fields
            .iter()
            .enumerate()
            .find_map(|(i, (n, t))| (n.as_str() == name).then(|| (i, t.clone())))
    }

    /// Wraps `target` in a chain of `Deref`s until it names a struct value,
    /// mirroring field-access auto-deref.
    fn autoderef_to_struct(&self, mut target: Expr) -> Expr {
        while let Ty::Ptr(inner) = target.ty.clone() {
            let pointee_ty = *inner;
            target = Expr {
                span: target.span,
                ty: pointee_ty,
                kind: ExprKind::Deref(Box::new(target)),
            };
        }
        target
    }

    /// For a UFCS receiver: if the function's first parameter is a pointer and
    /// the receiver is an addressable non-pointer, take its address.
    fn maybe_autoref(&self, recv: Expr, fn_ty: &Ty) -> Expr {
        if let Ty::Function { params, .. } = fn_ty
            && matches!(params.first(), Some(Ty::Ptr(_)))
            && !matches!(recv.ty, Ty::Ptr(_))
            && is_place_kind(&recv.kind)
        {
            let ty = Ty::Ptr(Box::new(recv.ty.clone()));
            let span = recv.span;
            return Expr {
                span,
                ty,
                kind: ExprKind::Ref(Box::new(recv)),
            };
        }
        recv
    }

    /// The parameter types of function `id`, or empty if it isn't a function.
    fn fn_params(&self, id: LocalId) -> Vec<Ty> {
        match self.bindings.get(&id) {
            Some(Ty::Function { params, .. }) => params.clone(),
            _ => Vec::new(),
        }
    }

    /// Method-call analogue: receiver-aware version of the closure two-pass.
    fn lower_method_call_args_with_closures(
        &mut self,
        name: &str,
        recv: &Expr,
        args: Vec<ast::Expr>,
    ) -> Result<Vec<Expr>, Error> {
        let n = args.len();
        let is_closure: Vec<bool> = args
            .iter()
            .map(|a| matches!(a.kind, ast::ExprKind::Closure { .. }))
            .collect();
        let mut slots: Vec<Option<Expr>> = (0..n).map(|_| None).collect();
        let mut pending: Vec<(usize, ast::Expr)> = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            if is_closure[i] {
                pending.push((i, a));
            } else {
                slots[i] = Some(self.lower_expr(a)?);
            }
        }
        let known: Vec<Option<Ty>> = slots
            .iter()
            .map(|s| s.as_ref().map(|e| e.ty.clone()))
            .collect();
        let hints = self.arg_hints_from_receiver(name, recv, &known);
        for (i, a) in pending {
            let hint = hints.as_ref().and_then(|h| h.get(i));
            slots[i] = Some(self.lower_expr_with_hint(a, hint)?);
        }
        Ok(slots.into_iter().map(|s| s.expect("filled")).collect())
    }

    /// Two-pass lowering of arguments for an overloaded direct call when some
    /// of them are closure literals: lower the non-closure args first, narrow
    /// candidates from their types via `arg_hints_from_args`, then lower
    /// the closures with the resulting per-position hints.
    fn lower_overloaded_call_args_with_closures(
        &mut self,
        name: &str,
        args: Vec<ast::Expr>,
    ) -> Result<Vec<Expr>, Error> {
        let n = args.len();
        let is_closure: Vec<bool> = args
            .iter()
            .map(|a| matches!(a.kind, ast::ExprKind::Closure { .. }))
            .collect();
        let mut slots: Vec<Option<Expr>> = (0..n).map(|_| None).collect();
        let mut pending: Vec<(usize, ast::Expr)> = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            if is_closure[i] {
                pending.push((i, a));
            } else {
                slots[i] = Some(self.lower_expr(a)?);
            }
        }
        let known: Vec<Option<Ty>> = slots
            .iter()
            .map(|s| s.as_ref().map(|e| e.ty.clone()))
            .collect();
        let hints = self.arg_hints_from_args(name, &known);
        for (i, a) in pending {
            let hint = hints.as_ref().and_then(|h| h.get(i));
            slots[i] = Some(self.lower_expr_with_hint(a, hint)?);
        }
        Ok(slots.into_iter().map(|s| s.expect("filled")).collect())
    }

    /// Whether `params` accept `arg_tys`: arity matches and each pair unifies,
    /// treating the parameters' type vars as inference holes.
    fn overload_matches(&self, params: &[Ty], arg_tys: &[Ty]) -> bool {
        if params.len() != arg_tys.len() {
            return false;
        }
        let mut subst = HashMap::new();
        for (p, a) in params.iter().zip(arg_tys.iter()) {
            // `unify(param, arg)` binds TypeVars on the param side. Inside a
            // generic function body the arg's *own* TypeVars also need to be
            // allowed to "match anything" — swap so unify always sees the
            // typevar-bearing side on the left.
            let (lhs, rhs) = if !ty_has_typevars(p) && ty_has_typevars(a) {
                (a, p)
            } else {
                (p, a)
            };
            if unify(
                lhs,
                rhs,
                Span::default(),
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )
            .is_err()
            {
                return false;
            }
        }
        true
    }

    /// Like `arg_hints_from_receiver` but without a receiver — pre-narrows
    /// overloads from already-lowered arg types alone. `None` entries mark
    /// not-yet-lowered positions (closures) and skip unification there.
    fn arg_hints_from_args(
        &mut self,
        name: &str,
        known_arg_tys: &[Option<Ty>],
    ) -> Option<Vec<Ty>> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return None;
        }
        let n_args = known_arg_tys.len();
        let mut matched: Vec<(Vec<Ty>, HashMap<TypeVarId, Ty>)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.len() != n_args {
                continue;
            }
            let mut subst = HashMap::new();
            let mut ok = true;
            for (param, known) in params.iter().zip(known_arg_tys.iter()) {
                let Some(arg_ty) = known else { continue };
                if !try_unify_one(
                    arg_ty,
                    param,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                ) {
                    ok = false;
                    break;
                }
            }
            if ok {
                matched.push((params.clone(), subst));
            }
        }
        if matched.len() != 1 {
            return None;
        }
        let (params, subst) = matched.into_iter().next().unwrap();
        Some(
            params
                .iter()
                .map(|p| self.substitute_ty(p, &subst))
                .collect(),
        )
    }

    /// Pre-narrows overloads of `name` using the receiver and (optionally)
    /// already-lowered other args. If exactly one candidate matches, returns
    /// its non-receiver param types (positions 1..N) with all derived
    /// TypeVars substituted. Holes in `known_other_arg_tys` (None) skip that
    /// position for unification — used for args not yet lowered, e.g. closures.
    fn arg_hints_from_receiver(
        &mut self,
        name: &str,
        recv: &Expr,
        known_other_arg_tys: &[Option<Ty>],
    ) -> Option<Vec<Ty>> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return None;
        }
        let forms = self.receiver_forms(recv);
        let n_args = known_other_arg_tys.len();
        let mut matched: Vec<(Vec<Ty>, HashMap<TypeVarId, Ty>)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.len() != n_args + 1 {
                continue;
            }
            for (form_ty, _adjust) in &forms {
                let mut subst = HashMap::new();
                if !try_unify_one(
                    form_ty,
                    &params[0],
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                ) {
                    continue;
                }
                let mut ok = true;
                for (param, known) in params.iter().skip(1).zip(known_other_arg_tys.iter()) {
                    let Some(arg_ty) = known else { continue };
                    if !try_unify_one(
                        arg_ty,
                        param,
                        &mut subst,
                        &self.struct_template_origin,
                        &self.enum_template_origin,
                    ) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    matched.push((params.clone(), subst));
                    break;
                }
            }
        }
        if matched.len() != 1 {
            return None;
        }
        let (params, subst) = matched.into_iter().next().unwrap();
        Some(
            params
                .iter()
                .skip(1)
                .map(|p| self.substitute_ty(p, &subst))
                .collect(),
        )
    }

    /// Picks the unique overload of `name` whose parameters accept `arg_tys`.
    fn resolve_overload(&self, name: &str, arg_tys: &[Ty], span: Span) -> Result<LocalId, Error> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        let mut matched: Vec<LocalId> = Vec::new();
        for id in candidates {
            if self.overload_matches(&self.fn_params(id), arg_tys) {
                matched.push(id);
            }
        }
        match matched.as_slice() {
            [id] => Ok(*id),
            [] => Err(Error {
                span,
                kind: ErrorKind::NoMatchingOverload {
                    name: name.to_string(),
                },
            }),
            _ => Err(Error {
                span,
                kind: ErrorKind::AmbiguousOverload {
                    name: name.to_string(),
                },
            }),
        }
    }

    /// Resolves a UFCS method `name`, choosing the overload that accepts the
    /// receiver (as-is, auto-referenced, or auto-dereferenced) plus the other
    /// arguments. Returns the chosen function and the adjusted receiver.
    fn resolve_method(
        &mut self,
        name: &str,
        recv: Expr,
        other_arg_tys: &[Ty],
        span: Span,
    ) -> Result<(LocalId, Expr), Error> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            // Not a top-level function — maybe a local holding a function value.
            let Some(id) = self.resolve(name) else {
                return Err(Error {
                    span,
                    kind: ErrorKind::NameNotFound {
                        name: name.to_string(),
                    },
                });
            };
            let fn_ty = self
                .bindings
                .get(&id)
                .cloned()
                .expect("resolved name must have a recorded type");
            let recv = self.maybe_autoref(recv, &fn_ty);
            return Ok((id, recv));
        }

        let forms = self.receiver_forms(&recv);
        let mut matched: Vec<(LocalId, RecvAdjust)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.is_empty() {
                continue;
            }
            for (form_ty, adjust) in &forms {
                let mut full = Vec::with_capacity(other_arg_tys.len() + 1);
                full.push(form_ty.clone());
                full.extend_from_slice(other_arg_tys);
                if self.overload_matches(&params, &full) {
                    matched.push((*id, *adjust));
                    break;
                }
            }
        }
        match matched.as_slice() {
            [(id, adjust)] => Ok((*id, self.apply_recv_adjust(recv, *adjust))),
            [] => Err(Error {
                span,
                kind: ErrorKind::NoMatchingOverload {
                    name: name.to_string(),
                },
            }),
            _ => Err(Error {
                span,
                kind: ErrorKind::AmbiguousOverload {
                    name: name.to_string(),
                },
            }),
        }
    }

    /// Receiver forms to try for UFCS dispatch, in preferred order: as-is, a
    /// reference (if addressable), then a dereference (if it's a pointer).
    fn receiver_forms(&self, recv: &Expr) -> Vec<(Ty, RecvAdjust)> {
        let mut forms = vec![(recv.ty.clone(), RecvAdjust::AsIs)];
        if is_place_kind(&recv.kind) {
            forms.push((Ty::Ptr(Box::new(recv.ty.clone())), RecvAdjust::Ref));
        }
        if let Ty::Ptr(inner) = &recv.ty {
            forms.push(((**inner).clone(), RecvAdjust::Deref));
        }
        forms
    }

    fn apply_recv_adjust(&self, recv: Expr, adjust: RecvAdjust) -> Expr {
        match adjust {
            RecvAdjust::AsIs => recv,
            RecvAdjust::Ref => {
                let ty = Ty::Ptr(Box::new(recv.ty.clone()));
                let span = recv.span;
                Expr {
                    span,
                    ty,
                    kind: ExprKind::Ref(Box::new(recv)),
                }
            }
            RecvAdjust::Deref => {
                let Ty::Ptr(inner) = recv.ty.clone() else {
                    unreachable!("Deref adjustment requires a pointer receiver");
                };
                let span = recv.span;
                Expr {
                    span,
                    ty: *inner,
                    kind: ExprKind::Deref(Box::new(recv)),
                }
            }
        }
    }

    fn lower_function_value(
        &mut self,
        params: Vec<ast::Param>,
        return_ty: Option<ast::TypeExpr>,
        body: ast::Block,
        span: crate::lexer::types::Span,
    ) -> Result<Expr, Error> {
        let return_ty = match return_ty {
            Some(t) => self.lower_type(&t)?,
            None => Ty::Unit,
        };

        self.enter_scope();

        let mut hir_params = Vec::new();
        let mut param_tys = Vec::new();
        for p in params {
            let ty = self.lower_type(&p.ty)?;
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(p.name.clone(), id);
            self.bindings.insert(id, ty.clone());
            param_tys.push(ty.clone());
            hir_params.push(Param {
                id,
                span: p.span,
                name: p.name,
                ty,
            });
        }

        let return_hint = (!ty_has_typevars(&return_ty)).then(|| return_ty.clone());
        self.return_ty_stack.push(return_ty.clone());
        let body_result = self.lower_block_with_hint(body, return_hint.as_ref());
        self.return_ty_stack.pop();
        let mut body = body_result?;
        if body.tail.ty != return_ty && return_ty.is_number() && return_ty != Ty::Int {
            let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
            body.tail = Box::new(coerce_through_tails(*tail, &return_ty)?);
        }

        self.leave_scope();

        let ty = Ty::Function {
            params: param_tys,
            return_ty: Box::new(return_ty.clone()),
            varargs: false,
        };

        let func = Function {
            params: hir_params,
            return_ty,
            body,
        };
        Ok(self.lift_function_value(func, ty, span))
    }

    /// Lowers a call's argument list with hints derived from the callee's
    /// parameter types. Closure args are deferred to a second pass so that
    /// non-closure args can extend the substitution first — without this
    /// step, `map<Int, Int>(Some(10), {x: x*2})` wouldn't see `Int` for `x`.
    fn lower_call_args(
        &mut self,
        callee: &Expr,
        args: Vec<ast::Expr>,
        explicit_type_args: &[Ty],
        _span: Span,
    ) -> Result<Vec<Expr>, Error> {
        let param_types: Vec<Ty> = match &callee.ty {
            Ty::Function { params, .. } => params.clone(),
            _ => return self.lower_args_no_hints(args),
        };

        // Seed substitution from the callee template (if any) and explicit
        // type args. This lets `map<Int, Int>(...)` immediately substitute T
        // and U in the param types before we hand them down as hints.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        if let ExprKind::Local(id) = &callee.kind
            && let Some(template) = self.templates.get(id)
            && explicit_type_args.len() == template.type_var_ids.len()
        {
            for (tv, ty) in template
                .type_var_ids
                .iter()
                .zip(explicit_type_args.iter())
            {
                subst.insert(*tv, ty.clone());
            }
        }

        let n = args.len();
        let mut args_opts: Vec<Option<ast::Expr>> = args.into_iter().map(Some).collect();
        let mut lowered: Vec<Option<Expr>> = (0..n).map(|_| None).collect();

        // Pass 1: lower non-closure args, extending subst as we go.
        for i in 0..n {
            let is_closure = matches!(
                args_opts[i].as_ref().map(|a| &a.kind),
                Some(ast::ExprKind::Closure { .. })
            );
            if is_closure {
                continue;
            }
            let arg = args_opts[i].take().unwrap();
            let param_ty = param_types.get(i).cloned();
            let hint = param_ty.as_ref().map(|p| self.substitute_ty(p, &subst));
            // Forward the hint even if it still has TypeVars — variant
            // constructors can emit a deferred (GenericEnum-typed)
            // EnumConstruct, and everyone else ignores the hint anyway.
            let v = self.lower_expr_with_hint(arg, hint.as_ref())?;
            if let Some(p) = &param_ty {
                // Best-effort: don't error here — finish_call/specialize_call
                // will produce the real diagnostic if the types don't fit.
                let _ = crate::hir::generics::unify(
                    p,
                    &v.ty,
                    v.span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                );
            }
            lowered[i] = Some(v);
        }

        // Pass 2: closures, with hints refined by pass 1's substitution.
        for i in 0..n {
            if lowered[i].is_some() {
                continue;
            }
            let arg = args_opts[i].take().unwrap();
            let param_ty = param_types.get(i).cloned();
            let hint = param_ty.as_ref().map(|p| self.substitute_ty(p, &subst));
            let v = self.lower_expr_with_hint(arg, hint.as_ref())?;
            lowered[i] = Some(v);
        }

        Ok(lowered.into_iter().map(Option::unwrap).collect())
    }

    fn lower_args_no_hints(&mut self, args: Vec<ast::Expr>) -> Result<Vec<Expr>, Error> {
        args.into_iter().map(|a| self.lower_expr(a)).collect()
    }

    /// Wraps a lowered `Function` in a synthesized top-level declaration so
    /// codegen sees only module-level functions. Returns a `Local` reference
    /// to the lift; callers use it like any other function value.
    ///
    /// Generic functions (signatures with TypeVars) register as templates
    /// instead, so the call site can specialize per-use exactly like a named
    /// top-level generic.
    fn lift_function_value(&mut self, func: Function, fn_ty: Ty, span: Span) -> Expr {
        let synth_id = self.id_gen.fresh();
        let synth_name = format!("__lambda_{}", synth_id.raw());
        self.bindings.insert(synth_id, fn_ty.clone());

        let is_generic = func.params.iter().any(|p| ty_has_typevars(&p.ty))
            || ty_has_typevars(&func.return_ty);
        if is_generic {
            let mut type_var_ids = Vec::new();
            for p in &func.params {
                collect_typevars(&p.ty, &mut type_var_ids);
            }
            collect_typevars(&func.return_ty, &mut type_var_ids);
            self.templates.insert(
                synth_id,
                GenericTemplate {
                    name: synth_name,
                    span,
                    type_var_ids,
                    params: func.params,
                    return_ty: func.return_ty,
                    body: func.body,
                },
            );
        } else {
            self.pending_specializations.push(Declaration {
                id: synth_id,
                span,
                name: synth_name,
                ty: fn_ty.clone(),
                value: Expr {
                    span,
                    ty: fn_ty.clone(),
                    kind: ExprKind::Function(func),
                },
            });
        }

        Expr {
            span,
            ty: fn_ty,
            kind: ExprKind::Local(synth_id),
        }
    }

    /// Lowers a closure literal `{x, y : body}` using `hint` as the expected
    /// function type. The body is lowered in a globals-only scope to forbid
    /// captures, then the result is lifted to a synthesized top-level fn.
    fn lower_closure(
        &mut self,
        params: Vec<String>,
        body: ast::Expr,
        hint: Option<&Ty>,
        span: Span,
    ) -> Result<Expr, Error> {
        let Some(Ty::Function {
            params: hint_params,
            return_ty: hint_return,
            ..
        }) = hint
        else {
            return Err(Error {
                span,
                kind: ErrorKind::ClosureTypeUnknown,
            });
        };
        if hint_params.len() != params.len() {
            return Err(Error {
                span,
                kind: ErrorKind::ClosureArityMismatch {
                    expected: hint_params.len(),
                    found: params.len(),
                },
            });
        }
        if hint_params.iter().any(ty_has_typevars) {
            return Err(Error {
                span,
                kind: ErrorKind::ClosureTypeUnknown,
            });
        }

        // Save scopes; lower the body seeing only globals + closure params.
        let saved_scopes = std::mem::replace(&mut self.scopes, Vec::new());
        let global_scope = saved_scopes[0].clone();
        self.scopes.push(global_scope);
        self.scopes.push(HashMap::new());

        let mut hir_params = Vec::with_capacity(params.len());
        for (name, ty) in params.into_iter().zip(hint_params.iter()) {
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(name.clone(), id);
            self.bindings.insert(id, ty.clone());
            hir_params.push(Param {
                id,
                span,
                name,
                ty: ty.clone(),
            });
        }

        let return_hint = if !ty_has_typevars(hint_return) {
            Some((**hint_return).clone())
        } else {
            None
        };
        let body_result = self.lower_expr_with_hint(body, return_hint.as_ref());
        self.scopes = saved_scopes;
        let body_expr = body_result?;

        let return_ty = body_expr.ty.clone();
        // Wrap the body expression in a single-tail block so codegen can
        // reuse its function-lowering path verbatim.
        let body_block = Block {
            span,
            items: Vec::new(),
            tail: Box::new(body_expr),
        };
        let fn_ty = Ty::Function {
            params: hir_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(return_ty.clone()),
            varargs: false,
        };
        let func = Function {
            params: hir_params,
            return_ty,
            body: body_block,
        };
        Ok(self.lift_function_value(func, fn_ty, span))
    }

    fn lower_block(&mut self, b: ast::Block) -> Result<Block, Error> {
        self.lower_block_with_hint(b, None)
    }

    /// `hint` flows into the block's tail expression only — intermediate
    /// statements don't see it.
    fn lower_block_with_hint(
        &mut self,
        b: ast::Block,
        hint: Option<&Ty>,
    ) -> Result<Block, Error> {
        let mut items = Vec::new();

        for item in b.items {
            match item {
                ast::BlockItem::Declaration(d) => {
                    // Nested function literals alias their name to a lifted
                    // top-level synth id and produce no block item.
                    if let Some(d) = self.lower_declaration(d)? {
                        items.push(BlockItem::Declaration(d));
                    }
                }
                ast::BlockItem::Statement(statement) => {
                    let s = self.lower_statement(statement)?;
                    items.push(BlockItem::Statement(s));
                }
            }
        }

        let tail = match b.tail {
            Some(expr) => Box::new(self.lower_expr_with_hint(*expr, hint)?),
            None => Box::new(unit_expr(end_of(b.span))),
        };

        Ok(Block {
            span: b.span,
            items,
            tail,
        })
    }

    fn lower_statement(&mut self, s: ast::Statement) -> Result<Statement, Error> {
        let kind = match s.kind {
            ast::StatementKind::Return(expr) => {
                let expr = match expr {
                    Some(e) => {
                        // Push the current function's return type as a hint
                        // so `return None;` and friends infer correctly. Even
                        // a TypeVar-bearing hint (e.g. `Option<V>` inside a
                        // generic body) is useful — variant constructors can
                        // defer their specialization through it.
                        let hint = self.return_ty_stack.last().cloned();
                        self.lower_expr_with_hint(e, hint.as_ref())?
                    }
                    None => unit_expr(end_of(s.span)),
                };
                StatementKind::Return(expr)
            }
            ast::StatementKind::Expr(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Expr(expr)
            }
            ast::StatementKind::Break => StatementKind::Break,
            ast::StatementKind::Defer(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Defer(expr)
            }
        };

        Ok(Statement { span: s.span, kind })
    }

    fn lower_intrinsic(
        &mut self,
        kind: IntrinsicKind,
        type_args: Vec<ast::TypeExpr>,
        args: Vec<ast::Expr>,
        span: Span,
    ) -> Result<Expr, Error> {
        // `alloc`/`realloc` need the element type for `sizeof`; `free` doesn't.
        let elem_ty = match type_args.first() {
            Some(t) => self.lower_type(t)?,
            None => {
                if matches!(kind, IntrinsicKind::Alloc | IntrinsicKind::Realloc) {
                    let name = match kind {
                        IntrinsicKind::Alloc => "alloc",
                        IntrinsicKind::Realloc => "realloc",
                        IntrinsicKind::Free => "free",
                    };
                    return Err(Error {
                        span,
                        kind: ErrorKind::MissingTypeArguments {
                            name: name.to_string(),
                        },
                    });
                }
                Ty::Unit
            }
        };
        let args: Vec<Expr> = args
            .into_iter()
            .map(|a| self.lower_expr(a))
            .collect::<Result<_, _>>()?;
        let ty = match kind {
            IntrinsicKind::Alloc | IntrinsicKind::Realloc => Ty::Ptr(Box::new(elem_ty.clone())),
            IntrinsicKind::Free => Ty::Unit,
        };
        Ok(Expr {
            span,
            ty,
            kind: ExprKind::Intrinsic {
                kind,
                elem_ty,
                args,
            },
        })
    }

    fn lower_type(&mut self, t: &ast::TypeExpr) -> Result<Ty, Error> {
        match &t.kind {
            ast::TypeExprKind::Int => Ok(Ty::Int),
            ast::TypeExprKind::UInt => Ok(Ty::UInt),
            ast::TypeExprKind::Float => Ok(Ty::Float),
            ast::TypeExprKind::I8 => Ok(Ty::I8),
            ast::TypeExprKind::I32 => Ok(Ty::I32),
            ast::TypeExprKind::I64 => Ok(Ty::I64),
            ast::TypeExprKind::U8 => Ok(Ty::U8),
            ast::TypeExprKind::U32 => Ok(Ty::U32),
            ast::TypeExprKind::U64 => Ok(Ty::U64),
            ast::TypeExprKind::F32 => Ok(Ty::F32),
            ast::TypeExprKind::F64 => Ok(Ty::F64),
            ast::TypeExprKind::Array { element_ty, count } => {
                let element = Box::new(self.lower_type(element_ty)?);
                Ok(Ty::Array {
                    element,
                    count: *count,
                })
            }
            ast::TypeExprKind::Function { params, return_ty } => {
                let params = params
                    .iter()
                    .map(|p| self.lower_type(p))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_ty = Box::new(self.lower_type(return_ty)?);
                Ok(Ty::Function {
                    params,
                    return_ty,
                    varargs: false,
                })
            }
            ast::TypeExprKind::Ptr(target) => {
                let target = Box::new(self.lower_type(target)?);
                Ok(Ty::Ptr(target))
            }
            ast::TypeExprKind::Named(name) => {
                if let Some(stripped) = name.strip_prefix('$') {
                    // `$T` — declares a fresh type variable in the current
                    // function's type-var scope. The same name appearing as
                    // bare `T` elsewhere in the same signature/body resolves
                    // to this same TypeVarId.
                    if self.structs.contains_key(stripped) {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::GenericIsAlsoAStruct {
                                name: stripped.to_string(),
                            },
                        });
                    }
                    let scope = self.type_var_scopes.last_mut().ok_or_else(|| Error {
                        span: t.span,
                        kind: ErrorKind::GenericOutsideFunctionSignature {
                            name: stripped.to_string(),
                        },
                    })?;
                    if scope.contains_key(stripped) {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::GenericAlreadyDefined {
                                name: stripped.to_string(),
                            },
                        });
                    }
                    let id = self.fresh_type_var_id(stripped.to_string());
                    self.type_var_scopes
                        .last_mut()
                        .unwrap()
                        .insert(stripped.to_string(), id);
                    Ok(Ty::TypeVar(id))
                } else if self.structs.contains_key(name) {
                    Ok(Ty::Struct(name.clone()))
                } else if self.enums.contains_key(name) {
                    Ok(Ty::Enum(name.clone()))
                } else if self.struct_templates.contains_key(name)
                    || self.enum_templates.contains_key(name)
                {
                    Err(Error {
                        span: t.span,
                        kind: ErrorKind::MissingTypeArguments { name: name.clone() },
                    })
                } else if let Some(id) = self.lookup_type_var(name) {
                    Ok(Ty::TypeVar(id))
                } else {
                    Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnknownType { name: name.clone() },
                    })
                }
            }
            ast::TypeExprKind::NamedGeneric { name, args } => {
                // Concrete struct/enum? Then `Foo<...>` is wrong — Foo isn't generic.
                if self.structs.contains_key(name) || self.enums.contains_key(name) {
                    return Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnexpectedTypeArguments { name: name.clone() },
                    });
                }
                if let Some(template) = self.struct_templates.get(name).cloned() {
                    if template.type_var_ids.len() != args.len() {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::TypeArgArityMismatch {
                                name: name.clone(),
                                expected: template.type_var_ids.len(),
                                found: args.len(),
                            },
                        });
                    }
                    let lowered_args: Vec<Ty> = args
                        .iter()
                        .map(|a| self.lower_type(a))
                        .collect::<Result<_, _>>()?;
                    let still_generic = lowered_args
                        .iter()
                        .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
                    if still_generic {
                        return Ok(Ty::GenericStruct {
                            name: name.clone(),
                            args: lowered_args,
                        });
                    }
                    let specialized = self.specialize_struct(&template, lowered_args, t.span)?;
                    return Ok(Ty::Struct(specialized));
                }
                if let Some(template) = self.enum_templates.get(name).cloned() {
                    if template.type_var_ids.len() != args.len() {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::TypeArgArityMismatch {
                                name: name.clone(),
                                expected: template.type_var_ids.len(),
                                found: args.len(),
                            },
                        });
                    }
                    let lowered_args: Vec<Ty> = args
                        .iter()
                        .map(|a| self.lower_type(a))
                        .collect::<Result<_, _>>()?;
                    let still_generic = lowered_args
                        .iter()
                        .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericEnum { .. }));
                    if still_generic {
                        return Ok(Ty::GenericEnum {
                            name: name.clone(),
                            args: lowered_args,
                        });
                    }
                    let specialized = self.specialize_enum(&template, lowered_args, t.span)?;
                    return Ok(Ty::Enum(specialized));
                }
                Err(Error {
                    span: t.span,
                    kind: ErrorKind::UnknownType { name: name.clone() },
                })
            }
            ast::TypeExprKind::Tuple(elems) => {
                let lowered: Vec<Ty> = elems
                    .iter()
                    .map(|e| self.lower_type(e))
                    .collect::<Result<_, _>>()?;
                Ok(Ty::Tuple(lowered))
            }
        }
    }

    fn lower_struct_literal(
        &mut self,
        def: StructDef,
        name: String,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            let target_ty = def
                .fields
                .iter()
                .find(|(n, _)| n == &f.name)
                .map(|(_, t)| t.clone());
            let Some(target_ty) = target_ty else {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            };
            let value = self.lower_expr(f.value)?;
            let value = coerce_int_literal(value, &target_ty)?;
            lowered_by_name.insert(f.name, value);
        }

        let mut missing = Vec::new();
        let mut ordered = Vec::with_capacity(def.fields.len());
        for (fname, _) in &def.fields {
            match lowered_by_name.remove(fname) {
                Some(v) => ordered.push((fname.clone(), v)),
                None => missing.push(fname.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        Ok(Expr {
            span,
            ty: Ty::Struct(def.name),
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    /// Lowers `Name<T, U> { ... }` — a generic struct literal with explicit
    /// type arguments. Unlike the inferred path, the type args come from the
    /// `<...>` list, so this works even when no field's value pins them (e.g.
    /// a phantom parameter). When an arg is still a type var (inside a generic
    /// body) the literal stays a `GenericStruct` and specializes later.
    fn lower_explicit_generic_struct_literal(
        &mut self,
        template: StructDef,
        name: String,
        type_args: Vec<ast::TypeExpr>,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        let args: Vec<Ty> = type_args
            .iter()
            .map(|t| self.lower_type(t))
            .collect::<Result<_, _>>()?;
        if args.len() != template.type_var_ids.len() {
            return Err(Error {
                span,
                kind: ErrorKind::TypeArgArityMismatch {
                    name,
                    expected: template.type_var_ids.len(),
                    found: args.len(),
                },
            });
        }
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        // Lower and coerce each field against its (substituted) declared type.
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            let Some((_, template_field_ty)) = template.fields.iter().find(|(n, _)| n == &f.name)
            else {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            };
            let field_ty = template_field_ty.clone();
            let target_ty = self.substitute_ty(&field_ty, &subst);
            let value = self.lower_expr(f.value)?;
            let value = coerce_int_literal(value, &target_ty)?;
            lowered_by_name.insert(f.name, value);
        }

        let mut missing = Vec::new();
        let mut ordered = Vec::with_capacity(template.fields.len());
        for (fname, _) in &template.fields {
            match lowered_by_name.remove(fname) {
                Some(v) => ordered.push((fname.clone(), v)),
                None => missing.push(fname.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        // Defer while any type argument is still a type var (we're inside a
        // generic body); otherwise specialize the struct now.
        let still_generic = args
            .iter()
            .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
        let ty = if still_generic {
            Ty::GenericStruct { name, args }
        } else {
            Ty::Struct(self.specialize_struct(&template, args, span)?)
        };

        Ok(Expr {
            span,
            ty,
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    fn lower_generic_struct_literal(
        &mut self,
        template: StructDef,
        name: String,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        // First pass: lower each provided field value (no coercion yet — the
        // template field type may be a TypeVar so we don't know what to coerce
        // to). Detect duplicate and unknown field names while we're here.
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        let mut spans_by_name: HashMap<String, Span> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            if !template.fields.iter().any(|(n, _)| n == &f.name) {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            // Coerce the value against the template's field type up front.
            // For TypeVar fields this is a no-op (TypeVar isn't a number);
            // for concrete fields it handles literal-to-numeric coercion so
            // unify doesn't have to reason about `Int → UInt`.
            let template_field_ty = template
                .fields
                .iter()
                .find(|(n, _)| n == &f.name)
                .map(|(_, t)| t.clone())
                .expect("checked above");
            let value = self.lower_expr(f.value)?;
            let value = coerce_int_literal(value, &template_field_ty)?;
            spans_by_name.insert(f.name.clone(), value.span);
            lowered_by_name.insert(f.name, value);
        }

        // Check no fields are missing — we need every value provided to
        // potentially help infer the type args.
        let mut missing = Vec::new();
        for (fname, _) in &template.fields {
            if !lowered_by_name.contains_key(fname) {
                missing.push(fname.clone());
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        // Second pass: unify each provided value's type against the template
        // field's type to deduce type arguments.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        for (fname, template_ty) in &template.fields {
            let value = lowered_by_name.get(fname).expect("checked above");
            let field_span = spans_by_name.get(fname).copied().unwrap_or(span);
            unify(
                template_ty,
                &value.ty,
                field_span,
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )?;
        }
        for tv in &template.type_var_ids {
            if !subst.contains_key(tv) {
                let tv_name = self
                    .type_vars
                    .get(tv.0 as usize)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| format!("T{}", tv.0));
                return Err(Error {
                    span,
                    kind: ErrorKind::CannotInferTypeArg { name: tv_name },
                });
            }
        }

        // Specialize and coerce values against the concrete field types.
        let args: Vec<Ty> = template
            .type_var_ids
            .iter()
            .map(|id| subst[id].clone())
            .collect();
        let specialized_name = self.specialize_struct(&template, args, span)?;
        let def = self
            .structs
            .get(&specialized_name)
            .cloned()
            .expect("just inserted by specialize_struct");

        let mut ordered = Vec::with_capacity(def.fields.len());
        for (fname, target_ty) in &def.fields {
            let value = lowered_by_name.remove(fname).expect("checked above");
            let value = coerce_int_literal(value, target_ty)?;
            ordered.push((fname.clone(), value));
        }

        Ok(Expr {
            span,
            ty: Ty::Struct(specialized_name),
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    /// Generates (or returns the cached) specialization of a generic struct
    /// `template` for the given concrete type arguments. The specialized
    /// struct lives in `self.structs` with a mangled name, ready for codegen.
    fn specialize_struct(
        &mut self,
        template: &StructDef,
        args: Vec<Ty>,
        _span: Span,
    ) -> Result<String, Error> {
        let cache_key = (template.name.clone(), args.clone());
        if let Some(cached) = self.struct_specialization_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let mangled = mangle_struct_specialization(&template.name, &args);
        // Reserve the cache slot BEFORE substituting so a self-referential
        // pointer field (`*Self<T>`) finds the in-progress entry.
        self.struct_specialization_cache
            .insert(cache_key, mangled.clone());
        self.struct_template_origin
            .insert(mangled.clone(), (template.name.clone(), args.clone()));

        let mut subst = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        let fields = template.fields.clone();
        let mut new_fields = Vec::with_capacity(fields.len());
        for (n, ty) in fields {
            new_fields.push((n, self.substitute_ty(&ty, &subst)));
        }

        self.structs.insert(
            mangled.clone(),
            StructDef {
                name: mangled.clone(),
                type_var_ids: Vec::new(),
                fields: new_fields,
            },
        );

        Ok(mangled)
    }

    /// Enum analogue of `specialize_struct`.
    fn specialize_enum(
        &mut self,
        template: &crate::hir::types::EnumDef,
        args: Vec<Ty>,
        _span: Span,
    ) -> Result<String, Error> {
        let cache_key = (template.name.clone(), args.clone());
        if let Some(cached) = self.enum_specialization_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let mangled = mangle_struct_specialization(&template.name, &args);
        self.enum_specialization_cache
            .insert(cache_key, mangled.clone());
        self.enum_template_origin
            .insert(mangled.clone(), (template.name.clone(), args.clone()));

        let mut subst = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        let variants = template.variants.clone();
        let mut new_variants = Vec::with_capacity(variants.len());
        for v in variants {
            let fields: Vec<Ty> = v
                .fields
                .iter()
                .map(|t| self.substitute_ty(t, &subst))
                .collect();
            new_variants.push(crate::hir::types::EnumVariant {
                name: v.name,
                fields,
            });
        }

        self.enums.insert(
            mangled.clone(),
            crate::hir::types::EnumDef {
                name: mangled.clone(),
                type_var_ids: Vec::new(),
                variants: new_variants,
            },
        );

        Ok(mangled)
    }

    /// Returns `Ok(None)` if `name` isn't a variant constructor — the caller
    /// then falls through to regular call lowering.
    fn try_lower_variant_call(
        &mut self,
        name: &str,
        type_args: &[ast::TypeExpr],
        args: Vec<ast::Expr>,
        span: Span,
        hint: Option<&Ty>,
    ) -> Result<Option<Expr>, Error> {
        let Some(&(ref enum_template_name, variant_index)) = self.variant_constructors.get(name)
        else {
            return Ok(None);
        };
        let enum_template_name = enum_template_name.clone();

        // Non-generic enum: variant fields are already concrete.
        if let Some(enum_def) = self.enums.get(&enum_template_name).cloned() {
            if !type_args.is_empty() {
                return Err(Error {
                    span,
                    kind: ErrorKind::UnexpectedTypeArguments {
                        name: enum_template_name.clone(),
                    },
                });
            }
            let variant = &enum_def.variants[variant_index];
            if variant.fields.len() != args.len() {
                return Err(Error {
                    span,
                    kind: ErrorKind::VariantArityMismatch {
                        name: name.to_string(),
                        expected: variant.fields.len(),
                        found: args.len(),
                    },
                });
            }
            let mut lowered_args = Vec::with_capacity(args.len());
            for (arg, field_ty) in args.into_iter().zip(variant.fields.iter()) {
                let v = self.lower_expr_with_hint(arg, Some(field_ty))?;
                let v = coerce_int_literal(v, field_ty)?;
                if v.ty != *field_ty {
                    return Err(Error {
                        span: v.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: field_ty.clone(),
                            found: v.ty.clone(),
                        },
                    });
                }
                lowered_args.push(v);
            }
            return Ok(Some(Expr {
                span,
                ty: Ty::Enum(enum_def.name.clone()),
                kind: ExprKind::EnumConstruct {
                    enum_name: enum_def.name,
                    variant_index,
                    args: lowered_args,
                },
            }));
        }

        // Generic enum: unify args + explicit type args to infer the
        // substitution, then specialize.
        let template = self
            .enum_templates
            .get(&enum_template_name)
            .cloned()
            .expect("variant_constructors points to a known template");
        let variant = &template.variants[variant_index];
        if variant.fields.len() != args.len() {
            return Err(Error {
                span,
                kind: ErrorKind::VariantArityMismatch {
                    name: name.to_string(),
                    expected: variant.fields.len(),
                    found: args.len(),
                },
            });
        }

        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        if !type_args.is_empty() {
            if type_args.len() != template.type_var_ids.len() {
                return Err(Error {
                    span,
                    kind: ErrorKind::TypeArgArityMismatch {
                        name: enum_template_name.clone(),
                        expected: template.type_var_ids.len(),
                        found: type_args.len(),
                    },
                });
            }
            for (tv, t) in template.type_var_ids.iter().zip(type_args.iter()) {
                let ty = self.lower_type(t)?;
                subst.insert(*tv, ty);
            }
        }
        // Hint seeding is only used as a last resort below (when no args are
        // present to drive unification, or when args left some TypeVars
        // unbound). Seeding eagerly here would block unify from later
        // refining a TypeVar hint (e.g. `Some(v.get(0))` inside a `map(...)`).

        // Lower args, coerce against (currently-substituted) field type, unify.
        let mut lowered_args = Vec::with_capacity(args.len());
        for (arg, field_ty) in args.into_iter().zip(variant.fields.iter()) {
            let coerce_target = self.substitute_ty(field_ty, &subst);
            let arg_hint = (!ty_has_typevars(&coerce_target)).then(|| coerce_target.clone());
            let v = self.lower_expr_with_hint(arg, arg_hint.as_ref())?;
            let v = coerce_int_literal(v, &coerce_target)?;
            crate::hir::generics::unify(
                field_ty,
                &v.ty,
                v.span,
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )?;
            lowered_args.push(v);
        }

        // Fall back to the hint for any slot that arg-unification didn't bind.
        let hint_args = enum_type_args_from_hint(
            &enum_template_name,
            hint,
            &self.enum_template_origin,
        );
        let mut resolved_args = Vec::with_capacity(template.type_var_ids.len());
        for (i, tv) in template.type_var_ids.iter().enumerate() {
            let t = subst.get(tv).cloned().or_else(|| {
                hint_args
                    .as_ref()
                    .filter(|args| args.len() == template.type_var_ids.len())
                    .map(|args| args[i].clone())
            });
            let Some(t) = t else {
                return Err(Error {
                    span,
                    kind: ErrorKind::CannotInferEnumTypeArg {
                        variant: name.to_string(),
                    },
                });
            };
            resolved_args.push(t);
        }

        // Inside a generic function body, type args may still be TypeVars.
        // Defer specialization to substitute_expr: emit an EnumConstruct
        // tagged with the template name and a generic enum type. The
        // post-specialization walk re-mangles it once args are concrete.
        if resolved_args.iter().any(ty_has_typevars) {
            return Ok(Some(Expr {
                span,
                ty: Ty::GenericEnum {
                    name: enum_template_name.clone(),
                    args: resolved_args,
                },
                kind: ExprKind::EnumConstruct {
                    enum_name: enum_template_name,
                    variant_index,
                    args: lowered_args,
                },
            }));
        }

        let mangled = self.specialize_enum(&template, resolved_args, span)?;
        let def = self
            .enums
            .get(&mangled)
            .expect("specialize_enum inserts the definition");
        let coerced_args: Vec<Expr> = lowered_args
            .into_iter()
            .zip(def.variants[variant_index].fields.iter())
            .map(|(v, target)| coerce_int_literal(v, target))
            .collect::<Result<_, _>>()?;
        Ok(Some(Expr {
            span,
            ty: Ty::Enum(mangled.clone()),
            kind: ExprKind::EnumConstruct {
                enum_name: mangled,
                variant_index,
                args: coerced_args,
            },
        }))
    }

    fn lower_match(
        &mut self,
        scrutinee: ast::Expr,
        arms: Vec<ast::MatchArm>,
        span: Span,
        hint: Option<&Ty>,
    ) -> Result<Expr, Error> {
        let scrutinee = self.lower_expr(scrutinee)?;
        // Inside a generic body the scrutinee is `GenericEnum<T>`; we match
        // against the template's variants with payload types substituted.
        // Specialization (later) updates binding types through substitute_expr.
        let (enum_def, subst): (crate::hir::types::EnumDef, HashMap<TypeVarId, Ty>) =
            match &scrutinee.ty {
                Ty::Enum(n) => (
                    self.enums
                        .get(n)
                        .cloned()
                        .expect("Ty::Enum must reference a known enum"),
                    HashMap::new(),
                ),
                Ty::GenericEnum { name, args } => {
                    let template = self
                        .enum_templates
                        .get(name)
                        .cloned()
                        .expect("GenericEnum must reference a known template");
                    let mut subst = HashMap::new();
                    for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
                        subst.insert(*tv, ty.clone());
                    }
                    let mut substituted = template.clone();
                    for v in &mut substituted.variants {
                        v.fields = v
                            .fields
                            .iter()
                            .map(|t| self.substitute_ty(t, &subst))
                            .collect();
                    }
                    (substituted, subst)
                }
                other => {
                    return Err(Error {
                        span: scrutinee.span,
                        kind: ErrorKind::MatchOnNonEnum {
                            found: other.clone(),
                        },
                    });
                }
            };
        let _ = subst; // Already applied to enum_def above.

        let mut hir_arms: Vec<crate::hir::types::MatchArm> = Vec::with_capacity(arms.len());
        let mut covered: Vec<bool> = vec![false; enum_def.variants.len()];
        let mut has_wildcard = false;
        let mut result_ty: Option<Ty> = None;

        for arm in arms {
            let arm_span = arm.span;
            let (pattern, binding_locals) =
                self.lower_pattern(arm.pattern, &enum_def, &mut covered, &mut has_wildcard)?;
            self.enter_scope();
            for (name, local_id, ty) in &binding_locals {
                self.current_scope_mut().insert(name.clone(), *local_id);
                self.bindings.insert(*local_id, ty.clone());
            }
            // Arm bodies see (in priority order): the first arm's type once
            // we have it, then the outer hint.
            let arm_hint = result_ty.as_ref().or(hint);
            let body = self.lower_expr_with_hint(arm.body, arm_hint);
            self.leave_scope();
            let body = body?;

            // Widen int-literal arms to match the first arm's type.
            let body = if let Some(rt) = &result_ty {
                coerce_through_tails(body, rt)?
            } else {
                body
            };

            if let Some(rt) = &result_ty {
                if body.ty != *rt {
                    return Err(Error {
                        span: body.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: rt.clone(),
                            found: body.ty.clone(),
                        },
                    });
                }
            } else {
                result_ty = Some(body.ty.clone());
            }

            hir_arms.push(crate::hir::types::MatchArm {
                span: arm_span,
                pattern,
                body,
            });
        }

        if !has_wildcard {
            let missing: Vec<String> = enum_def
                .variants
                .iter()
                .zip(covered.iter())
                .filter_map(|(v, &c)| (!c).then(|| v.name.clone()))
                .collect();
            if !missing.is_empty() {
                return Err(Error {
                    span,
                    kind: ErrorKind::NonExhaustiveMatch { missing },
                });
            }
        }

        let ty = result_ty.unwrap_or(Ty::Unit);
        Ok(Expr {
            span,
            ty,
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms: hir_arms,
            },
        })
    }

    fn lower_pattern(
        &mut self,
        pattern: ast::Pattern,
        enum_def: &crate::hir::types::EnumDef,
        covered: &mut [bool],
        has_wildcard: &mut bool,
    ) -> Result<(crate::hir::types::HirPattern, Vec<(String, LocalId, Ty)>), Error> {
        let p_span = pattern.span;
        match pattern.kind {
            ast::PatternKind::Wildcard => {
                if *has_wildcard {
                    return Err(Error {
                        span: p_span,
                        kind: ErrorKind::DuplicateMatchArm {
                            name: "_".to_string(),
                        },
                    });
                }
                *has_wildcard = true;
                Ok((crate::hir::types::HirPattern::Wildcard, Vec::new()))
            }
            ast::PatternKind::Binding(name) => {
                // A bare identifier names a variant; catch-all bindings use `_`.
                self.resolve_variant_pattern(&name, Vec::new(), enum_def, covered, p_span)
            }
            ast::PatternKind::Variant { name, bindings } => {
                self.resolve_variant_pattern(&name, bindings, enum_def, covered, p_span)
            }
        }
    }

    fn resolve_variant_pattern(
        &mut self,
        name: &str,
        bindings: Vec<String>,
        enum_def: &crate::hir::types::EnumDef,
        covered: &mut [bool],
        p_span: Span,
    ) -> Result<(crate::hir::types::HirPattern, Vec<(String, LocalId, Ty)>), Error> {
        let (idx, variant) = enum_def
            .variants
            .iter()
            .enumerate()
            .find(|(_, v)| v.name == name)
            .ok_or_else(|| Error {
                span: p_span,
                kind: ErrorKind::UnknownVariant {
                    name: name.to_string(),
                },
            })?;
        if variant.fields.len() != bindings.len() {
            return Err(Error {
                span: p_span,
                kind: ErrorKind::VariantArityMismatch {
                    name: name.to_string(),
                    expected: variant.fields.len(),
                    found: bindings.len(),
                },
            });
        }
        if covered[idx] {
            return Err(Error {
                span: p_span,
                kind: ErrorKind::DuplicateMatchArm {
                    name: name.to_string(),
                },
            });
        }
        covered[idx] = true;
        let mut binding_locals = Vec::with_capacity(bindings.len());
        for (binding_name, field_ty) in bindings.iter().zip(variant.fields.iter()) {
            let local_id = self.id_gen.fresh();
            binding_locals.push((binding_name.clone(), local_id, field_ty.clone()));
        }
        let bindings_for_hir: Vec<(String, LocalId, Ty)> = binding_locals.clone();
        Ok((
            crate::hir::types::HirPattern::Variant {
                enum_name: enum_def.name.clone(),
                variant_index: idx,
                bindings: bindings_for_hir,
            },
            binding_locals,
        ))
    }

    fn substitute_ty(&mut self, ty: &Ty, subst: &HashMap<TypeVarId, Ty>) -> Ty {
        match ty {
            Ty::TypeVar(id) => subst.get(id).cloned().unwrap_or(Ty::TypeVar(*id)),
            Ty::Ptr(inner) => Ty::Ptr(Box::new(self.substitute_ty(inner, subst))),
            Ty::Array { element, count } => Ty::Array {
                element: Box::new(self.substitute_ty(element, subst)),
                count: *count,
            },
            Ty::Function {
                params,
                return_ty,
                varargs,
            } => {
                let params: Vec<Ty> = params
                    .iter()
                    .map(|p| self.substitute_ty(p, subst))
                    .collect();
                let return_ty = Box::new(self.substitute_ty(return_ty, subst));
                Ty::Function {
                    params,
                    return_ty,
                    varargs: *varargs,
                }
            }
            Ty::GenericStruct { name, args } => {
                let new_args: Vec<Ty> = args.iter().map(|a| self.substitute_ty(a, subst)).collect();
                let still_generic = new_args
                    .iter()
                    .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
                if still_generic {
                    Ty::GenericStruct {
                        name: name.clone(),
                        args: new_args,
                    }
                } else {
                    let template = self
                        .struct_templates
                        .get(name)
                        .cloned()
                        .expect("GenericStruct must reference a known template");
                    let specialized = self
                        .specialize_struct(&template, new_args, Span::default())
                        .expect("specialize_struct cannot fail on concrete args");
                    Ty::Struct(specialized)
                }
            }
            Ty::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.substitute_ty(e, subst)).collect())
            }
            Ty::GenericEnum { name, args } => {
                let new_args: Vec<Ty> = args.iter().map(|a| self.substitute_ty(a, subst)).collect();
                let still_generic = new_args
                    .iter()
                    .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericEnum { .. }));
                if still_generic {
                    Ty::GenericEnum {
                        name: name.clone(),
                        args: new_args,
                    }
                } else {
                    let template = self
                        .enum_templates
                        .get(name)
                        .cloned()
                        .expect("GenericEnum must reference a known template");
                    let specialized = self
                        .specialize_enum(&template, new_args, Span::default())
                        .expect("specialize_enum cannot fail on concrete args");
                    Ty::Enum(specialized)
                }
            }
            _ => ty.clone(),
        }
    }

    fn lower_unary(&self, op: ast::UnaryOperator) -> UnaryOperator {
        match op {
            ast::UnaryOperator::Not => UnaryOperator::Not,
            ast::UnaryOperator::Minus => UnaryOperator::Minus,
        }
    }

    /// Produces a specialization of the template identified by `template_id`
    /// for the given concrete argument types, returning the LocalId of the
    /// specialized function. The id is reserved up front and cached so
    /// recursive calls inside the template body resolve to the same id.
    fn specialize_call(
        &mut self,
        template_id: LocalId,
        arg_tys: &[Ty],
        explicit_type_args: &[Ty],
        call_span: crate::lexer::types::Span,
    ) -> Result<LocalId, Error> {
        let template = self
            .templates
            .get(&template_id)
            .expect("specialize_call: template_id is not a generic template")
            .clone();

        // Build subst by unifying each (param, arg) pair.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();

        // Seed bindings from explicit type arguments, if provided.
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != template.type_var_ids.len() {
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::TypeArgArityMismatch {
                        name: template.name.clone(),
                        expected: template.type_var_ids.len(),
                        found: explicit_type_args.len(),
                    },
                });
            }
            for (tv, ty) in template.type_var_ids.iter().zip(explicit_type_args.iter()) {
                subst.insert(*tv, ty.clone());
            }
        }

        // For a call site we unify each arg against the corresponding param to
        // extend the substitution. For a function-reference specialization
        // there are no args — the explicit type args alone must cover all
        // type vars, and the check below catches it. So skip arg-unification
        // entirely when arg_tys is empty.
        if !arg_tys.is_empty() {
            if template.params.len() != arg_tys.len() {
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::TypeMismatch {
                        expected: Ty::Function {
                            params: template.params.iter().map(|p| p.ty.clone()).collect(),
                            return_ty: Box::new(template.return_ty.clone()),
                            varargs: false,
                        },
                        found: Ty::Function {
                            params: arg_tys.to_vec(),
                            return_ty: Box::new(Ty::Unit),
                            varargs: false,
                        },
                    },
                });
            }
            for (param, arg_ty) in template.params.iter().zip(arg_tys.iter()) {
                unify(
                    &param.ty,
                    arg_ty,
                    call_span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                )?;
            }
        }

        // Make sure every type var declared by the template is bound.
        for tv_id in &template.type_var_ids {
            if !subst.contains_key(tv_id) {
                let name = self
                    .type_vars
                    .get(tv_id.0 as usize)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| format!("T{}", tv_id.0));
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::CannotInferTypeArg { name },
                });
            }
        }

        let key_tys: Vec<Ty> = template
            .type_var_ids
            .iter()
            .map(|id| subst[id].clone())
            .collect();
        let cache_key = (template_id, key_tys.clone());
        if let Some(&cached) = self.specialization_cache.get(&cache_key) {
            return Ok(cached);
        }

        // Reserve the new id BEFORE walking the body so self-recursive calls
        // find the in-progress entry instead of re-specializing forever.
        let new_id = self.id_gen.fresh();
        self.specialization_cache.insert(cache_key, new_id);

        // Build the specialization signature.
        let mut new_params = Vec::with_capacity(template.params.len());
        let template_params = template.params.clone();
        for p in &template_params {
            let new_param_id = self.id_gen.fresh();
            let ty = self.substitute_ty(&p.ty, &subst);
            new_params.push(Param {
                id: new_param_id,
                span: p.span,
                name: p.name.clone(),
                ty,
            });
        }
        let new_return_ty = self.substitute_ty(&template.return_ty, &subst);
        let new_fn_ty = Ty::Function {
            params: new_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(new_return_ty.clone()),
            varargs: false,
        };
        self.bindings.insert(new_id, new_fn_ty.clone());

        // Walk and substitute the body. References to template params (by
        // old LocalId) get remapped to the new param LocalIds.
        let mut new_body = template.body.clone();
        let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();
        for (p_old, p_new) in template_params.iter().zip(new_params.iter()) {
            local_map.insert(p_old.id, p_new.id);
        }
        // Comptime templates fold their type-introspection during this walk.
        let is_comptime = self.comptime_template_ids.contains(&template_id);
        let prev_comptime_subst = self.in_comptime_subst;
        self.in_comptime_subst = is_comptime;
        let subst_result = self.substitute_block(&mut new_body, &subst, &mut local_map);
        self.in_comptime_subst = prev_comptime_subst;
        subst_result?;

        // Build the specialization's Declaration. Compute the mangled name
        // up front so we don't keep borrowing `template.name` while
        // mutably calling substitute_block.
        let template_name = template.name.clone();
        let template_span = template.span;
        drop(template);

        let mangled = mangle_specialization(&template_name, &key_tys);
        let value = Expr {
            span: template_span,
            ty: new_fn_ty.clone(),
            kind: ExprKind::Function(Function {
                params: new_params,
                return_ty: new_return_ty,
                body: new_body,
            }),
        };
        let decl = Declaration {
            id: new_id,
            span: template_span,
            name: mangled,
            ty: new_fn_ty,
            value,
        };
        self.pending_specializations.push(decl);

        Ok(new_id)
    }

    fn substitute_block(
        &mut self,
        block: &mut Block,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        if self.in_comptime_subst {
            // Process items until one diverges, then drop everything after it
            // (including the tail) as unreachable. This is what stops a
            // fall-through `comperror` from firing once an earlier static-if
            // branch has been selected and returns.
            let mut new_items = Vec::new();
            let mut diverged = false;
            for mut item in std::mem::take(&mut block.items) {
                self.substitute_block_item(&mut item, subst, local_map)?;
                let d = crate::hir::comptime::item_diverges(&item);
                new_items.push(item);
                if d {
                    diverged = true;
                    break;
                }
            }
            block.items = new_items;
            if diverged {
                block.tail = Box::new(unit_expr(block.span));
            } else {
                self.substitute_expr(&mut block.tail, subst, local_map)?;
            }
            return Ok(());
        }

        for item in &mut block.items {
            self.substitute_block_item(item, subst, local_map)?;
        }
        self.substitute_expr(&mut block.tail, subst, local_map)?;
        Ok(())
    }

    fn substitute_block_item(
        &mut self,
        item: &mut BlockItem,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        match item {
            BlockItem::Declaration(d) => {
                d.ty = self.substitute_ty(&d.ty, subst);
                self.substitute_expr(&mut d.value, subst, local_map)?;
                // Local declarations introduce a fresh LocalId in the
                // specialization to keep ids distinct from the template's.
                let new_id = self.id_gen.fresh();
                local_map.insert(d.id, new_id);
                d.id = new_id;
                self.bindings.insert(new_id, d.ty.clone());
            }
            BlockItem::Statement(s) => match &mut s.kind {
                StatementKind::Return(e) => self.substitute_expr(e, subst, local_map)?,
                StatementKind::Expr(e) => self.substitute_expr(e, subst, local_map)?,
                StatementKind::Break => {}
                StatementKind::Defer(e) => self.substitute_expr(e, subst, local_map)?,
            },
        }
        Ok(())
    }

    fn substitute_expr(
        &mut self,
        e: &mut Expr,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        let span = e.span;

        // Comptime `static if`: when the condition is comptime-known, fold to
        // the taken branch and discard the other, so its dead code (including
        // any `comperror`) is never evaluated.
        if self.in_comptime_subst {
            let mut replacement: Option<Expr> = None;
            if let ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } = &mut e.kind
            {
                self.substitute_expr(condition, subst, local_map)?;
                if let ExprKind::Const(Const::Int(n)) = &condition.kind {
                    let n = *n;
                    let taken = if n != 0 { then_branch } else { else_branch };
                    replacement = Some(std::mem::replace(taken.as_mut(), unit_expr(span)));
                }
            }
            if let Some(mut taken) = replacement {
                self.substitute_expr(&mut taken, subst, local_map)?;
                *e = taken;
                return Ok(());
            }
        }

        e.ty = self.substitute_ty(&e.ty, subst);
        // When a nested call is re-specialized, we need to update the Call
        // expression's result type to match the new specialization's return
        // type (its old ty was the template's return_ty, which may still
        // contain TypeVars that this substitution didn't bind).
        let mut updated_call_ty: Option<Ty> = None;
        // When a comptime comparison folds to a constant, stash it and rewrite
        // `e` once the `&mut e.kind` borrow below has ended.
        let mut comptime_const_fold: Option<i32> = None;
        match &mut e.kind {
            ExprKind::Const(_) => {}
            ExprKind::Local(id) => {
                if let Some(&new) = local_map.get(id) {
                    *id = new;
                }
            }
            ExprKind::Function(_) => {
                // Nested function literals aren't supported as generic
                // values; leave alone. If their body uses outer TypeVars
                // they'd need their own walk, but that's not on the table.
            }
            ExprKind::Call(call) => {
                self.substitute_expr(&mut call.callee, subst, local_map)?;
                for arg in &mut call.args {
                    self.substitute_expr(arg, subst, local_map)?;
                }
                // If the callee resolves to a generic template AND every arg
                // is concrete now, re-specialize.
                if let ExprKind::Local(callee_id) = call.callee.kind
                    && self.templates.contains_key(&callee_id)
                {
                    let arg_tys: Vec<Ty> = call.args.iter().map(|a| a.ty.clone()).collect();
                    if !arg_tys.iter().any(ty_has_typevars) {
                        let new_id = self.specialize_call(callee_id, &arg_tys, &[], e.span)?;
                        let new_fn_ty = self.bindings[&new_id].clone();
                        if let Ty::Function { return_ty, .. } = &new_fn_ty {
                            updated_call_ty = Some((**return_ty).clone());
                        }
                        call.callee.kind = ExprKind::Local(new_id);
                        call.callee.ty = new_fn_ty;
                    }
                }
            }
            ExprKind::Cast { target, expr } => {
                *target = self.substitute_ty(target, subst);
                self.substitute_expr(expr, subst, local_map)?;
            }
            ExprKind::Unary { operand, .. } => self.substitute_expr(operand, subst, local_map)?,
            ExprKind::Block(b) => self.substitute_block(b, subst, local_map)?,
            ExprKind::Binary { op, lhs, rhs } => {
                self.substitute_expr(lhs, subst, local_map)?;
                self.substitute_expr(rhs, subst, local_map)?;
                if self.in_comptime_subst
                    && let Some(crate::hir::comptime::CtValue::Bool(b)) =
                        crate::hir::comptime::eval_binary(*op, lhs, rhs)
                {
                    comptime_const_fold = Some(if b { 1 } else { 0 });
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.substitute_expr(condition, subst, local_map)?;
                self.substitute_expr(then_branch, subst, local_map)?;
                self.substitute_expr(else_branch, subst, local_map)?;
            }
            ExprKind::While { condition, body } => {
                self.substitute_expr(condition, subst, local_map)?;
                self.substitute_block(body, subst, local_map)?;
            }
            ExprKind::Assign { target, value } => {
                self.substitute_expr(target, subst, local_map)?;
                self.substitute_expr(value, subst, local_map)?;
            }
            ExprKind::Array(items) => {
                for item in items {
                    self.substitute_expr(item, subst, local_map)?;
                }
            }
            ExprKind::Subscript { expr, index } => {
                self.substitute_expr(expr, subst, local_map)?;
                self.substitute_expr(index, subst, local_map)?;
            }
            ExprKind::Ref(t) => self.substitute_expr(t, subst, local_map)?,
            ExprKind::Deref(t) => self.substitute_expr(t, subst, local_map)?,
            ExprKind::StructLiteral { fields } => {
                for (_, value) in fields {
                    self.substitute_expr(value, subst, local_map)?;
                }
            }
            ExprKind::Field { target, .. } => self.substitute_expr(target, subst, local_map)?,
            ExprKind::TypeValue(ty) => {
                *ty = self.substitute_ty(ty, subst);
            }
            ExprKind::CompError(message) => {
                return Err(Error {
                    span,
                    kind: ErrorKind::ComptimeError {
                        message: message.clone(),
                    },
                });
            }
            ExprKind::Intrinsic { elem_ty, args, .. } => {
                *elem_ty = self.substitute_ty(elem_ty, subst);
                for arg in args {
                    self.substitute_expr(arg, subst, local_map)?;
                }
            }
            ExprKind::Tuple(elems) => {
                for el in elems {
                    self.substitute_expr(el, subst, local_map)?;
                }
            }
            ExprKind::TupleField { target, .. } => {
                self.substitute_expr(target, subst, local_map)?;
            }
            ExprKind::EnumConstruct {
                enum_name,
                variant_index,
                args,
            } => {
                for arg in args.iter_mut() {
                    self.substitute_expr(arg, subst, local_map)?;
                }
                // If e.ty resolved to a concrete Ty::Enum (substitute_ty above
                // handles GenericEnum→Enum), point enum_name at the
                // specialization and coerce int-literal args against the now-
                // concrete variant field types.
                if let Ty::Enum(mangled) = &e.ty {
                    *enum_name = mangled.clone();
                    let variant_fields = self
                        .enums
                        .get(mangled)
                        .expect("substitute_ty registered this enum")
                        .variants[*variant_index]
                        .fields
                        .clone();
                    for (arg, target) in args.iter_mut().zip(variant_fields.iter()) {
                        let taken = std::mem::replace(arg, unit_expr(arg.span));
                        *arg = coerce_int_literal(taken, target)?;
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.substitute_expr(scrutinee, subst, local_map)?;
                for arm in arms {
                    if let crate::hir::types::HirPattern::Variant { bindings, .. } =
                        &mut arm.pattern
                    {
                        for (_, _, ty) in bindings {
                            *ty = self.substitute_ty(ty, subst);
                        }
                    }
                    self.substitute_expr(&mut arm.body, subst, local_map)?;
                }
            }
            ExprKind::ZeroInit(ty) => {
                *ty = self.substitute_ty(ty, subst);
            }
            ExprKind::ExternFunction { .. } => {
                // Extern decls never appear inside a generic body — they're
                // always top-level — so substitution can't reach them.
                unreachable!("ExternFunction never substitutes");
            }
            ExprKind::DeferredFunctionRef {
                template_id,
                type_args,
            } => {
                let substituted: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.substitute_ty(t, subst))
                    .collect();
                if substituted.iter().any(ty_has_typevars) {
                    *type_args = substituted;
                } else {
                    let span = e.span;
                    let specialized = self.specialize_call(
                        *template_id,
                        &[],
                        &substituted,
                        span,
                    )?;
                    let fn_ty = self.bindings[&specialized].clone();
                    e.kind = ExprKind::Local(specialized);
                    e.ty = fn_ty;
                }
            }
        }
        if let Some(ty) = updated_call_ty {
            e.ty = ty;
        }
        if let Some(n) = comptime_const_fold {
            e.kind = ExprKind::Const(Const::Int(n));
            e.ty = Ty::Int;
        }
        Ok(())
    }
}

/// How a UFCS receiver is adjusted to match the chosen overload's first param.
#[derive(Clone, Copy)]
enum RecvAdjust {
    AsIs,
    Ref,
    Deref,
}

/// Unifies `arg_ty` against `param_ty`, growing `subst` in place. Same
/// orientation-swap trick as `overload_matches`: whichever side has TypeVars
/// is treated as the inference side.
fn try_unify_one(
    arg_ty: &Ty,
    param_ty: &Ty,
    subst: &mut HashMap<TypeVarId, Ty>,
    struct_template_origin: &HashMap<String, (String, Vec<Ty>)>,
    enum_template_origin: &HashMap<String, (String, Vec<Ty>)>,
) -> bool {
    let (lhs, rhs) = if !ty_has_typevars(param_ty) && ty_has_typevars(arg_ty) {
        (arg_ty, param_ty)
    } else {
        (param_ty, arg_ty)
    };
    unify(
        lhs,
        rhs,
        Span::default(),
        subst,
        struct_template_origin,
        enum_template_origin,
    )
    .is_ok()
}

/// Whether an expression denotes an addressable place (so `&expr` is valid).
fn is_place_kind(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Local(_)
            | ExprKind::Field { .. }
            | ExprKind::Subscript { .. }
            | ExprKind::Deref(_)
    )
}

/// Reads the type-argument list off a hint shaped like `template_name<...>`,
/// otherwise None. Args may still contain TypeVars (caller decides whether
/// to specialize now or defer through substitute_expr).
fn enum_type_args_from_hint(
    template_name: &str,
    hint: Option<&Ty>,
    enum_origins: &HashMap<String, (String, Vec<Ty>)>,
) -> Option<Vec<Ty>> {
    let hint = hint?;
    match hint {
        Ty::Enum(spec) => enum_origins
            .get(spec)
            .filter(|(tname, _)| tname == template_name)
            .map(|(_, args)| args.clone()),
        Ty::GenericEnum { name, args } if name == template_name => Some(args.clone()),
        _ => None,
    }
}

fn mangle_specialization(name: &str, tys: &[Ty]) -> String {
    let mut s = String::from(name);
    for t in tys {
        s.push('$');
        s.push_str(&format!("{t:?}"));
    }
    s
}

fn mangle_struct_specialization(name: &str, tys: &[Ty]) -> String {
    mangle_specialization(name, tys)
}

impl Lower {
    fn current_scope(&self) -> &HashMap<String, LocalId> {
        self.scopes.last().unwrap()
    }
    fn current_scope_mut(&mut self) -> &mut HashMap<String, LocalId> {
        self.scopes.last_mut().unwrap()
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn leave_scope(&mut self) {
        self.scopes.pop();
    }
    fn resolve(&self, name: &str) -> Option<LocalId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }
}

fn unit_expr(span: crate::lexer::types::Span) -> Expr {
    Expr {
        span,
        ty: Ty::Unit,
        kind: ExprKind::Const(Const::Unit),
    }
}

fn end_of(span: crate::lexer::types::Span) -> crate::lexer::types::Span {
    crate::lexer::types::Span {
        start: span.end,
        end: span.end,
    }
}
