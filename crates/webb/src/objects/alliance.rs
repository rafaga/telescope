//! The [`Alliance`] entity.

use super::catalog::BasicCatalog;

#[derive(Clone, PartialEq, Debug)]
pub struct Alliance {
    pub id: i64,
    pub name: String,
}

impl Alliance {
    #[tracing::instrument]
    pub fn new() -> Self {
        Alliance {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Alliance {
    #[tracing::instrument]
    fn default() -> Self {
        Self::new()
    }
}

impl BasicCatalog for Alliance {
    type Output = i64;

    #[tracing::instrument]
    fn id(&self) -> Self::Output {
        self.id
    }

    #[tracing::instrument]
    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alliance_new_is_zeroed() {
        let alliance = Alliance::new();
        assert_eq!(alliance.id, 0);
        assert_eq!(alliance.name, "");
        assert_eq!(alliance, Alliance::default());
    }

    #[test]
    fn alliance_implements_basic_catalog() {
        let alliance = Alliance {
            id: 99000001,
            name: String::from("Acme Alliance"),
        };
        assert_eq!(BasicCatalog::id(&alliance), 99000001);
        assert_eq!(BasicCatalog::name(&alliance), "Acme Alliance");
    }
}
