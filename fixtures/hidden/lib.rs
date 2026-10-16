pub const HIDDEN_MAX: usize = 3;

pub fn hidden_max() -> usize {
    HIDDEN_MAX
}

#[cfg(test)]
const HIDDEN_DOUBLE: usize = HIDDEN_MAX * 2;

#[cfg(test)]
mod tests {
    use super::HIDDEN_MAX;

    fn max_is_three() -> bool {
        HIDDEN_MAX == 3
    }
}
