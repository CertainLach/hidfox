pub use enum_variants_procedural::EnumVariants;

pub trait EnumVariants {
	/// Current variant name
	fn name(&self) -> &'static str;
	/// All variant names
	fn names() -> &'static [&'static str];
}
