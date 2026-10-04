use crate::utils::which;

pub fn apt_available() -> bool {
    which("apt-get").is_some()
}
