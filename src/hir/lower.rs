use std::collections::{HashMap, HashSet};

use crate::{
    ast,
    hir::{
        TypeVar, TypeVarId, UnaryOperator,
        coerce::coerce_through_tails,
        error::{Error, ErrorKind},
        generics::{collect_typevars, ty_has_typevars},
        types::{
            Block, Const, Declaration, Expr, ExprKind, Function, LocalId, LocalIdGen, Param, Program,
            StructDef, Ty,
        },
    },
    lexer::types::Span,
};

#[derive(Clone)]
pub(super) struct PendingFnSig {
    pub(super) scope: HashMap<String, TypeVarId>,
    /// Type-var ids in declaration order. Mirrors what the eventual
    /// `GenericTemplate.type_var_ids` will hold once the body is lowered.
    pub(super) type_var_ids: Vec<TypeVarId>,
    pub(super) param_tys: Vec<Ty>,
    pub(super) return_ty: Ty,
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
    pub(super) scopes: Vec<HashMap<String, LocalId>>,
    pub(super) bindings: HashMap<LocalId, Ty>,
    pub(super) structs: HashMap<String, StructDef>,
    pub(super) id_gen: LocalIdGen,

    pub(super) type_vars: Vec<TypeVar>,
    pub(super) type_var_scopes: Vec<HashMap<String, TypeVarId>>,
    pub(super) pending_fn_sigs: HashMap<LocalId, PendingFnSig>,
    /// Declaration-order type-var ids per function, captured during
    /// pre-registration. Outlives `pending_fn_sigs` (which is consumed when
    /// the body lowers) so `register_if_generic_template` can still see the
    /// original arity for `<$T, $U>` even when the signature mentions only
    /// some of them — the rest may still appear in the body.
    pub(super) fn_decl_type_var_ids: HashMap<LocalId, Vec<TypeVarId>>,
    /// Stack of "the return type of the function whose body we're lowering
    /// right now". `return e;` reads the top to push it as a hint to `e`,
    /// so `return None;` infers from the surrounding function's return type.
    pub(super) return_ty_stack: Vec<Ty>,

    pub(crate) templates: HashMap<LocalId, GenericTemplate>,

    // Cache: (template id, concrete arg types in TypeVarId order) →
    // specialization LocalId. The id is reserved BEFORE the specialization's
    // body is processed, so recursive calls find the in-progress entry.
    pub(super) specialization_cache: HashMap<(LocalId, Vec<Ty>), LocalId>,
    pub(super) pending_specializations: Vec<Declaration>,

    pub(super) struct_templates: HashMap<String, StructDef>,
    pub(super) struct_specialization_cache: HashMap<(String, Vec<Ty>), String>,
    pub(super) struct_template_origin: HashMap<String, (String, Vec<Ty>)>,

    // Mirror the struct registries above.
    pub(super) enums: HashMap<String, crate::hir::types::EnumDef>,
    pub(super) enum_templates: HashMap<String, crate::hir::types::EnumDef>,
    pub(super) enum_specialization_cache: HashMap<(String, Vec<Ty>), String>,
    pub(super) enum_template_origin: HashMap<String, (String, Vec<Ty>)>,
    /// Variant name → (template enum, tag index). Names are globally unique.
    pub(super) variant_constructors: HashMap<String, (String, usize)>,

    /// Ids of generic templates that came from `#comptime` declarations. When
    /// one is specialized, the substitution pass folds comptime constructs
    /// (type comparisons, `static if`) and drops unreachable code.
    pub(super) comptime_template_ids: HashSet<LocalId>,
    /// True while lowering a `#comptime` function body — enables lowering of
    /// `TypeValue` / `comperror` constructs that are illegal elsewhere.
    pub(super) in_comptime: bool,
    /// True while substituting a comptime template's body — enables folding.
    pub(super) in_comptime_subst: bool,

    /// Top-level function names → every declaration sharing that name (an
    /// overload set, resolved by argument types at the call site).
    pub(super) overloads: HashMap<String, Vec<LocalId>>,
    /// AST declaration node id → its pre-registered LocalId, so a function body
    /// finds its own id even when the name is overloaded.
    pub(super) fn_id_by_node: HashMap<crate::ast::NodeId, LocalId>,
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

    pub(super) fn push_type_var_scope(&mut self) {
        self.type_var_scopes.push(HashMap::new());
    }
    pub(super) fn pop_type_var_scope(&mut self) -> HashMap<String, TypeVarId> {
        self.type_var_scopes
            .pop()
            .expect("type_var_scopes underflow")
    }
    pub(super) fn lookup_type_var(&self, name: &str) -> Option<TypeVarId> {
        for s in self.type_var_scopes.iter().rev() {
            if let Some(&id) = s.get(name) {
                return Some(id);
            }
        }
        None
    }
    pub(super) fn fresh_type_var_id(&mut self, name: String) -> TypeVarId {
        let id = TypeVarId(self.type_vars.len() as u32);
        self.type_vars.push(TypeVar { name });
        id
    }

    /// Declares each name from a `<$K, $V>` clause as a fresh type variable in
    /// the current type-var scope, so later bare references (`K`, `V`) resolve.
    /// A type-var scope must already be pushed.
    pub(super) fn declare_type_params(&mut self, names: &[String], span: Span) -> Result<(), Error> {
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
    pub(super) fn register_if_generic_template(&mut self, decl: &Declaration) -> bool {
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

    pub(super) fn pre_register_top_level(&mut self, d: &ast::Declaration) -> Result<(), Error> {
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

    pub(super) fn lower_declaration(&mut self, d: ast::Declaration) -> Result<Option<Declaration>, Error> {
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

    pub(super) fn lower_top_level_function_body(
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

    pub(super) fn fn_params(&self, id: LocalId) -> Vec<Ty> {
        match self.bindings.get(&id) {
            Some(Ty::Function { params, .. }) => params.clone(),
            _ => Vec::new(),
        }
    }

    /// Method-call analogue: receiver-aware version of the closure two-pass.

    pub(super) fn lower_unary(&self, op: ast::UnaryOperator) -> UnaryOperator {
        match op {
            ast::UnaryOperator::Not => UnaryOperator::Not,
            ast::UnaryOperator::Minus => UnaryOperator::Minus,
        }
    }


}

pub(super) fn is_place_kind(kind: &ExprKind) -> bool {
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

pub(super) fn mangle_specialization(name: &str, tys: &[Ty]) -> String {
    let mut s = String::from(name);
    for t in tys {
        s.push('$');
        s.push_str(&format!("{t:?}"));
    }
    s
}


pub(super) fn mangle_struct_specialization(name: &str, tys: &[Ty]) -> String {
    mangle_specialization(name, tys)
}


impl Lower {
    pub(super) fn current_scope(&self) -> &HashMap<String, LocalId> {
        self.scopes.last().unwrap()
    }
    pub(super) fn current_scope_mut(&mut self) -> &mut HashMap<String, LocalId> {
        self.scopes.last_mut().unwrap()
    }

    pub(super) fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(super) fn leave_scope(&mut self) {
        self.scopes.pop();
    }
    pub(super) fn resolve(&self, name: &str) -> Option<LocalId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }
}

pub(super) fn unit_expr(span: crate::lexer::types::Span) -> Expr {
    Expr {
        span,
        ty: Ty::Unit,
        kind: ExprKind::Const(Const::Unit),
    }
}

pub(super) fn end_of(span: crate::lexer::types::Span) -> crate::lexer::types::Span {
    crate::lexer::types::Span {
        start: span.end,
        end: span.end,
    }
}
