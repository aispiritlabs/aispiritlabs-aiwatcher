//! Storage ownership is outside a lab's bytes and its version id.
//!
//! This binds storage, not authority: a caller checks a fresh IAM grant, and a
//! bound registry is never a substitute for one. A workshop **is** a project,
//! so who may read a lab is the grant on that project and there is no second
//! notion of access here to invent.

use aiwatcher_iam::ProjectScope;

use crate::{LabError, Registry, Result};

impl Registry {
    /// Bind every lab operation to one project without changing version ids.
    ///
    /// Identical content in two projects has the same version id and separate
    /// objects, which is the point: knowing an id grants nothing.
    ///
    /// # Errors
    ///
    /// [`LabError::InvalidScope`] when this registry is already bound to
    /// another project — a bound registry may be reopened in its own scope,
    /// never rebound — or when the configured prefix is not one a scope can
    /// safely be appended to.
    pub fn for_project(&self, scope: ProjectScope) -> Result<Self> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(LabError::InvalidScope(
                    "registry is already bound to another project",
                ))
            };
        }
        let mut config = self.config.clone();
        config.prefix = format!(
            "{}/scopes/{}/{}/registry",
            config.prefix.trim_end_matches('/'),
            scope.organization.0,
            scope.project.0
        );
        let registry = Self {
            store: self.store.clone(),
            config,
            scope: Some(scope),
        };
        registry.check_key(&format!("{}/scope-check", registry.config.prefix))?;
        Ok(registry)
    }

    /// The scope this registry is bound to, when it is bound to one.
    #[must_use]
    pub fn scope(&self) -> Option<ProjectScope> {
        self.scope
    }

    pub(crate) fn check_key(&self, key: &str) -> Result<()> {
        if self.scope.is_some()
            && (!key.starts_with(&format!("{}/", self.config.prefix))
                || key
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..") || part.contains('\\')))
        {
            return Err(LabError::InvalidScope(
                "object key escapes the project registry",
            ));
        }
        Ok(())
    }
}
