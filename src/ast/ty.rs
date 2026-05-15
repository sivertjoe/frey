use crate::ast::types::NodeId;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Int,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "int"),
        }
    }
}

#[derive(Default)]
pub struct TypeTable {
    types: HashMap<NodeId, Ty>,
}

impl TypeTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, id: NodeId, ty: Ty) {
        self.types.insert(id, ty);
    }

    pub fn get(&self, id: NodeId) -> Option<&Ty> {
        self.types.get(&id)
    }
}
