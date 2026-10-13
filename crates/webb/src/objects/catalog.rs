//! The [`BasicCatalog`] trait shared by the catalog-like EVE entities.

pub trait BasicCatalog {
    type Output;

    fn id(&self) -> Self::Output;
    fn name(&self) -> &str;
}
