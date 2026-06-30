/// An entry in the uasset name table
#[derive(Debug, Clone)]
pub struct FNameEntry {
    /// The name string
    pub name: String,
    /// Name flags (non-zero for some special names)
    pub flags: u32,
}
