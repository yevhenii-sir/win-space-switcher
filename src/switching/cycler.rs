use super::Direction;

/// Walks the full list from `current` in `direction` and returns the first eligible layout, or `current`
/// when there is nowhere else to go. Walking the full list (rather than only eligible layouts) means a
/// non-eligible current layout still leads to its nearest neighbour.
pub fn next_layout(layouts: &[isize], current: isize, direction: Direction, is_eligible: impl Fn(isize) -> bool) -> isize {
    let count = layouts.len() as isize;
    let start = layouts.iter().position(|&layout| layout == current).unwrap_or(0) as isize;

    (1..count)
        .map(|step| layouts[(start + direction.step() * step).rem_euclid(count) as usize])
        .find(|&candidate| is_eligible(candidate))
        .unwrap_or(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: isize = 1;
    const RU: isize = 2;
    const UK: isize = 3;
    const LAYOUTS: [isize; 3] = [EN, RU, UK];

    fn any(_: isize) -> bool {
        true
    }

    fn not_uk(layout: isize) -> bool {
        layout != UK
    }

    #[test]
    fn forward_moves_to_next_and_wraps() {
        assert_eq!(next_layout(&LAYOUTS, EN, Direction::Forward, any), RU);
        assert_eq!(next_layout(&LAYOUTS, RU, Direction::Forward, any), UK);
        assert_eq!(next_layout(&LAYOUTS, UK, Direction::Forward, any), EN);
    }

    #[test]
    fn backward_wraps_to_last() {
        assert_eq!(next_layout(&LAYOUTS, EN, Direction::Backward, any), UK);
    }

    #[test]
    fn skips_ineligible_layouts() {
        assert_eq!(next_layout(&LAYOUTS, RU, Direction::Forward, not_uk), EN);
    }

    #[test]
    fn from_ineligible_current_goes_to_nearest_eligible_in_direction() {
        assert_eq!(next_layout(&LAYOUTS, UK, Direction::Forward, not_uk), EN);
        assert_eq!(next_layout(&LAYOUTS, UK, Direction::Backward, not_uk), RU);
    }

    #[test]
    fn stays_when_nothing_else_is_eligible() {
        assert_eq!(next_layout(&LAYOUTS, EN, Direction::Forward, |layout| layout == EN), EN);
    }

    #[test]
    fn unknown_current_starts_from_first() {
        assert_eq!(next_layout(&LAYOUTS, 42, Direction::Forward, any), RU);
    }

    #[test]
    fn empty_list_keeps_current() {
        assert_eq!(next_layout(&[], EN, Direction::Forward, any), EN);
    }
}
