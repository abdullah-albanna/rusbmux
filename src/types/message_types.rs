#[derive(Debug, Clone, Copy)]
pub enum MessageTypes {
    Listen,
    ListDevice,
    ListListeners,
    ReadBUID,
    ReadPairRecord,
    SavePairRecord,
    DeletePairRecord,
}

impl std::fmt::Display for MessageTypes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listen => write!(f, "Listen"),
            Self::ListDevice => write!(f, "ListDevice"),
            Self::ListListeners => write!(f, "ListListeners"),
            Self::ReadBUID => write!(f, "ReadBUID"),
            Self::ReadPairRecord => write!(f, "ReadPairRecord"),
            Self::SavePairRecord => write!(f, "SavePairRecord"),
            Self::DeletePairRecord => write!(f, "DeletePairRecord"),
        }
    }
}
