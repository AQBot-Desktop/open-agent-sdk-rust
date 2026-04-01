use crate::types::Tool;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Create a registry with all default built-in tools.
    pub fn default_registry() -> Self {
        let mut registry = Self::new();

        // File I/O tools
        registry.register(Arc::new(super::bash::BashTool));
        registry.register(Arc::new(super::fileread::FileReadTool));
        registry.register(Arc::new(super::filewrite::FileWriteTool));
        registry.register(Arc::new(super::fileedit::FileEditTool));
        registry.register(Arc::new(super::glob_tool::GlobTool));
        registry.register(Arc::new(super::grep::GrepTool));

        // Web tools
        registry.register(Arc::new(super::webfetch::WebFetchTool));
        registry.register(Arc::new(super::websearch::WebSearchTool::default()));

        // User interaction
        registry.register(Arc::new(super::askuser::AskUserTool::default()));

        // Task tools
        let task_store = super::tasks::TaskStore::new();
        registry.register(Arc::new(super::tasks::TaskCreateTool::new(
            task_store.clone(),
        )));
        registry.register(Arc::new(super::tasks::TaskGetTool::new(
            task_store.clone(),
        )));
        registry.register(Arc::new(super::tasks::TaskListTool::new(
            task_store.clone(),
        )));
        registry.register(Arc::new(super::tasks::TaskUpdateTool::new(task_store)));

        // Tool search
        registry.register(Arc::new(super::toolsearch::ToolSearchTool::default()));

        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn filter<F>(&self, predicate: F) -> Vec<Arc<dyn Tool>>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .values()
            .filter(|t| predicate(t.as_ref()))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Remove tools by name.
    pub fn remove(&mut self, names: &[&str]) {
        for name in names {
            self.tools.remove(*name);
        }
    }

    /// Keep only the specified tools.
    pub fn retain(&mut self, names: &[&str]) {
        let name_set: std::collections::HashSet<&str> = names.iter().copied().collect();
        self.tools.retain(|k, _| name_set.contains(k.as_str()));
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::default_registry()
    }
}
