//! Interned name types for AADL identifiers.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::fmt;

/// An interned AADL identifier.
///
/// AADL identifiers are case-insensitive per the spec (AS5506 §3.1).
/// We store the original text but compare case-insensitively.
#[derive(Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Name(SmolStr);

impl Name {
    pub fn new(s: &str) -> Self {
        Self(SmolStr::new(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-insensitive equality (AADL spec).
    pub fn eq_ci(&self, other: &Name) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.0)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self(SmolStr::new(&s))
    }
}

/// A qualified classifier reference: `Package::Type` or `Package::Type.Impl`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClassifierRef {
    /// Package qualifier (if any).
    pub package: Option<Name>,
    /// Component type name.
    pub type_name: Name,
    /// Implementation name (if any — the part after the dot).
    pub impl_name: Option<Name>,
}

impl ClassifierRef {
    pub fn type_only(name: Name) -> Self {
        Self {
            package: None,
            type_name: name,
            impl_name: None,
        }
    }

    pub fn qualified(package: Name, type_name: Name) -> Self {
        Self {
            package: Some(package),
            type_name,
            impl_name: None,
        }
    }

    pub fn implementation(package: Option<Name>, type_name: Name, impl_name: Name) -> Self {
        Self {
            package,
            type_name,
            impl_name: Some(impl_name),
        }
    }

    /// Is this a reference to an implementation (has `.impl` suffix)?
    pub fn is_implementation(&self) -> bool {
        self.impl_name.is_some()
    }
}

impl fmt::Display for ClassifierRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(pkg) = &self.package {
            write!(f, "{}::", pkg)?;
        }
        write!(f, "{}", self.type_name)?;
        if let Some(imp) = &self.impl_name {
            write!(f, ".{}", imp)?;
        }
        Ok(())
    }
}

/// A property reference: `PropertySet::PropertyName`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PropertyRef {
    /// Property set qualifier (if any).
    pub property_set: Option<Name>,
    /// Property name.
    pub property_name: Name,
}

impl fmt::Display for PropertyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ps) = &self.property_set {
            write!(f, "{}::", ps)?;
        }
        write!(f, "{}", self.property_name)
    }
}
