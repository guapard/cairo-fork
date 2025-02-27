use std::any::Any;
use std::ops::Deref;
use std::sync::Arc;

use cairo_lang_filesystem::ids::DiagnosticMapping;
use cairo_lang_syntax::node::ast;
use cairo_lang_syntax::node::db::SyntaxGroup;
use cairo_lang_syntax::node::ids::SyntaxStablePtrId;
use cairo_lang_utils::ordered_hash_map::OrderedHashMap;
use smol_str::SmolStr;

/// A trait for arbitrary data that a macro generates along with a generated file.
pub trait GeneratedFileAuxData: std::fmt::Debug + Sync + Send {
    fn as_any(&self) -> &dyn Any;
    fn eq(&self, other: &dyn GeneratedFileAuxData) -> bool;
}

#[derive(Clone, Debug)]
pub struct DynGeneratedFileAuxData(pub Arc<dyn GeneratedFileAuxData>);
impl DynGeneratedFileAuxData {
    pub fn new<T: GeneratedFileAuxData + 'static>(aux_data: T) -> Self {
        DynGeneratedFileAuxData(Arc::new(aux_data))
    }
}
impl Deref for DynGeneratedFileAuxData {
    type Target = Arc<dyn GeneratedFileAuxData>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl PartialEq for DynGeneratedFileAuxData {
    fn eq(&self, that: &DynGeneratedFileAuxData) -> bool {
        GeneratedFileAuxData::eq(&*self.0, &*that.0)
    }
}
impl Eq for DynGeneratedFileAuxData {}

/// Virtual code file generated by a plugin.
pub struct PluginGeneratedFile {
    /// Name for the virtual file. Will appear in diagnostics.
    pub name: SmolStr,
    /// Code content for the file.
    pub content: String,
    /// A diagnostics mapper, to allow more readable diagnostics that originate in plugin generated
    /// virtual files.
    pub diagnostics_mappings: Vec<DiagnosticMapping>,
    /// Arbitrary data that the plugin generates along with the file.
    pub aux_data: Option<DynGeneratedFileAuxData>,
}

/// Result of plugin code generation.
#[derive(Default)]
pub struct PluginResult {
    /// Filename, content.
    pub code: Option<PluginGeneratedFile>,
    /// Diagnostics.
    pub diagnostics: Vec<PluginDiagnostic>,
    /// If true - the original item should be removed, if false - it should remain as is.
    pub remove_original_item: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PluginDiagnostic {
    pub stable_ptr: SyntaxStablePtrId,
    pub message: String,
}

// TOD(spapini): Move to another place.
/// A trait for a macro plugin: external plugin that generates additional code for items.
pub trait MacroPlugin: std::fmt::Debug + Sync + Send {
    /// Generates code for an item. If no code should be generated returns None.
    /// Otherwise, returns (virtual_module_name, module_content), and a virtual submodule
    /// with that name and content should be created.
    fn generate_code(&self, db: &dyn SyntaxGroup, item_ast: ast::Item) -> PluginResult;

    /// Attributes this plugin uses.
    /// Attributes the plugin uses without declaring here are likely to cause a compilation error
    /// for unknown attribute.
    /// Note: They may not cause a diagnostic if some other plugin declares such attribute, but
    /// plugin writers should not rely on that.
    fn declared_attributes(&self) -> Vec<String>;
}

/// Result of plugin code generation.
#[derive(Default)]
pub struct InlinePluginResult {
    pub code: Option<PluginGeneratedFile>,
    /// Diagnostics.
    pub diagnostics: Vec<PluginDiagnostic>,
}

pub trait InlineMacroExprPlugin: std::fmt::Debug + Sync + Send {
    /// Generates code for an item. If no code should be generated returns None.
    /// Otherwise, returns (virtual_module_name, module_content), and a virtual submodule
    /// with that name and content should be created.
    fn generate_code(
        &self,
        db: &dyn SyntaxGroup,
        item_ast: &ast::ExprInlineMacro,
    ) -> InlinePluginResult;
}

/// A trait for easier addition of macro plugins.
pub trait NamedPlugin: Default + 'static {
    const NAME: &'static str;
}

/// A suite of plugins.
#[derive(Clone, Debug, Default)]
pub struct PluginSuite {
    /// The macro plugins, running on all items.
    pub plugins: Vec<Arc<dyn MacroPlugin>>,
    /// The inline macro plugins, running on matching inline macro expressions.
    pub inline_macro_plugins: OrderedHashMap<String, Arc<dyn InlineMacroExprPlugin>>,
}
impl PluginSuite {
    /// Adds a macro plugin.
    pub fn add_plugin_ex(&mut self, plugin: Arc<dyn MacroPlugin>) -> &mut Self {
        self.plugins.push(plugin);
        self
    }
    /// Adds a macro plugin.
    pub fn add_plugin<T: MacroPlugin + Default + 'static>(&mut self) -> &mut Self {
        self.add_plugin_ex(Arc::new(T::default()))
    }
    /// Adds an inline macro plugin.
    pub fn add_inline_macro_plugin_ex(
        &mut self,
        name: &str,
        plugin: Arc<dyn InlineMacroExprPlugin>,
    ) -> &mut Self {
        self.inline_macro_plugins.insert(name.into(), plugin);
        self
    }
    /// Adds an inline macro plugin.
    pub fn add_inline_macro_plugin<T: NamedPlugin + InlineMacroExprPlugin>(&mut self) -> &mut Self {
        self.add_inline_macro_plugin_ex(T::NAME, Arc::new(T::default()));
        self
    }
    /// Adds another plugin suite into this suite.
    pub fn add(&mut self, suite: PluginSuite) -> &mut Self {
        self.plugins.extend(suite.plugins);
        self.inline_macro_plugins.extend(suite.inline_macro_plugins);
        self
    }
}
