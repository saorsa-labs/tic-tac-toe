//! Registry of available company templates.
//!
//! The registry is built once from the built-in templates (parsed + validated)
//! and exposed as a [`std::sync::LazyLock`]. Lookups by id return a cloned,
//! validated [`CompanyTemplate`].

use std::sync::LazyLock;

use crate::company_template::builtin::builtin_templates;
use crate::company_template::parse::parse_company_template;
use crate::company_template::spec::CompanyTemplate;
#[cfg(test)]
use crate::company_template::spec::TemplateSummary;

/// The built-in template registry, parsed + validated once at first use.
pub static REGISTRY: LazyLock<Vec<CompanyTemplate>> = LazyLock::new(|| {
    builtin_templates()
        .iter()
        .filter_map(|(_, toml)| parse_company_template(toml).ok())
        .collect()
});

/// List summaries of every available template, ordered by id.
#[cfg(test)]
pub fn list_company_templates() -> Vec<TemplateSummary> {
    let mut items: Vec<TemplateSummary> = REGISTRY.iter().map(TemplateSummary::from).collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

/// Look up a template by id.
pub fn get_company_template(id: &str) -> Option<CompanyTemplate> {
    REGISTRY.iter().find(|t| t.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_software_dev_and_sales() {
        let list = list_company_templates();
        assert!(list.iter().any(|s| s.id == "software-dev-and-sales"));
        let tpl = get_company_template("software-dev-and-sales").unwrap();
        assert_eq!(tpl.group_count(), 3);
        assert!(tpl.role_count() >= 4);
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(get_company_template("does-not-exist").is_none());
    }
}
