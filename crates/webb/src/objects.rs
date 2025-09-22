use chrono::prelude::*;
use rusqlite::Error;

pub trait EsiObject {
    fn retrieve() -> Result<bool, Error>;
}

pub enum TelescopeDbError {
    NoConnection,
}

#[derive(Clone, PartialEq)]
pub struct AuthData {
    pub token: String,
    pub expiration: Option<DateTime<Utc>>,
    pub refresh_token:String,
}

impl AuthData{
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        AuthData {
            token: String::new(),
            expiration: None,
            refresh_token: String::new()
        }
    }
}

impl Default for AuthData {
    fn default() -> Self {
         Self::new()
    }
}

#[derive(Clone, PartialEq)]
pub struct Character {
    pub id: i32,
    pub name: String,
    pub last_logon: DateTime<Utc>,
    pub corp: Option<Corporation>,
    pub alliance: Option<Alliance>,
    pub photo: Option<String>,
    pub location: i32,
}

impl Character {
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Character {
            id: 0,
            name: String::new(),
            last_logon: DateTime::default(),
            corp: None,
            alliance: None,
            photo: None,
            location: 0,
        }
    }
}

impl Default for Character {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Corporation {
    pub id: i32,
    pub name: String,
}

impl Corporation {
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Corporation {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Corporation {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

impl BasicCatalog for Corporation {
    type Output = i32;

    fn id(&self) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.id
    }

    fn name(&self) -> &str {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        &self.name
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Alliance {
    pub id: i32,
    pub name: String,
}

impl Alliance {
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Alliance {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Alliance {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

impl BasicCatalog for Alliance {
    type Output = i32;

    fn id(&self) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.id
    }

    fn name(&self) -> &str {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        &self.name
    }
}

pub trait BasicCatalog {
    type Output;

    fn id(&self) -> Self::Output;
    fn name(&self) -> &str;
}
