//! The [`Corporation`] entity.

use super::catalog::BasicCatalog;

#[derive(Clone, PartialEq, Debug)]
pub struct Corporation {
    pub id: i64,
    pub name: String,
}

impl Corporation {
    #[tracing::instrument]
    pub fn new() -> Self {
        Corporation {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Corporation {
    #[tracing::instrument]
    fn default() -> Self {
        Self::new()
    }
}

impl BasicCatalog for Corporation {
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
    fn corporation_new_is_zeroed() {
        let corp = Corporation::new();
        assert_eq!(corp.id, 0);
        assert_eq!(corp.name, "");
        assert_eq!(corp, Corporation::default());
    }

    #[test]
    fn corporation_implements_basic_catalog() {
        let corp = Corporation {
            id: 98000001,
            name: String::from("Acme Corp"),
        };
        assert_eq!(BasicCatalog::id(&corp), 98000001);
        assert_eq!(BasicCatalog::name(&corp), "Acme Corp");
    }
}
