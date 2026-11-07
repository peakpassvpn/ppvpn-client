//! OS peer metadata, populated only from a verified audit token. Not a wire DTO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub pid: u32,
    pub pid_version: u32,
    pub audit_session: u32,
}
