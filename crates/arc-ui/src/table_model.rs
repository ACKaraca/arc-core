//! The pure model of a table's column order and sort keys (CORE-UI-013).
//!
//! The order is a permutation of the drawn columns: entry `p` is the source column index drawn
//! at visual position `p`. The sort keys are an ordered list of columns with directions, at most
//! [`MAX_SORT_KEYS`] long. Every operation here works on plain values, so it is tested without a
//! user interface; [`crate::TableView`] applies them to its state.

use std::cmp::Ordering;

use arc_base::{Error, Result};

use crate::error::codes;
use crate::table::{Sort, SortDirection};

/// The most sort keys a table holds.
pub const MAX_SORT_KEYS: usize = 4;

/// The order of `columns` columns after the source changed: the entries of `order` that are
/// still valid keep their relative order, duplicates and vanished indices are dropped, and the
/// columns `order` does not name follow in ascending index order. The result is always a
/// permutation of `0..columns`.
#[must_use]
pub fn reconcile_order(order: &[usize], columns: usize) -> Vec<usize> {
    let mut named = vec![false; columns];
    let mut result = Vec::with_capacity(columns);
    for &column in order {
        if let Some(seen) = named.get_mut(column)
            && !*seen
        {
            *seen = true;
            result.push(column);
        }
    }
    for (column, seen) in named.iter().enumerate() {
        if !seen {
            result.push(column);
        }
    }
    result
}

/// Moves the entry at position `from` so that it ends at position `to`: the entries between the
/// two positions shift by one. Moving to the same position changes nothing.
///
/// # Errors
///
/// `ui.table.column_out_of_range` when `from` or `to` is not a position of `order`.
pub fn move_column(order: &mut [usize], from: usize, to: usize) -> Result<()> {
    if from >= order.len() || to >= order.len() {
        return Err(Error::new(codes::TABLE_COLUMN_OUT_OF_RANGE));
    }
    match from.cmp(&to) {
        Ordering::Less => {
            if let Some(run) = order.get_mut(from..=to) {
                run.rotate_left(1);
            }
        }
        Ordering::Greater => {
            if let Some(run) = order.get_mut(to..=from) {
                run.rotate_right(1);
            }
        }
        Ordering::Equal => {}
    }
    Ok(())
}

/// A plain click on the header of `column` (of `columns` columns): the keys become that single
/// key. Its direction is the opposite of the direction the column has now, ascending when it has
/// none. Returns the new key.
///
/// # Errors
///
/// `ui.table.column_out_of_range` when `column` is not below `columns`.
pub fn sort_single(keys: &mut Vec<Sort>, columns: usize, column: usize) -> Result<Sort> {
    check_column(columns, column)?;
    let direction = keys
        .iter()
        .find(|key| key.column == column)
        .map_or(SortDirection::Ascending, |key| reversed(key.direction));
    let key = Sort { column, direction };
    keys.clear();
    keys.push(key);
    Ok(key)
}

/// A modified click on the header of `column`: a key the column already has turns to the
/// opposite direction in its place; otherwise the column is added as a secondary ascending key.
/// Returns the key that changed or was added.
///
/// # Errors
///
/// `ui.table.column_out_of_range` when `column` is not below `columns`, and
/// `ui.table.sort_too_many` when the column needs a new key and [`MAX_SORT_KEYS`] are in use.
pub fn sort_additive(keys: &mut Vec<Sort>, columns: usize, column: usize) -> Result<Sort> {
    check_column(columns, column)?;
    if let Some(key) = keys.iter_mut().find(|key| key.column == column) {
        key.direction = reversed(key.direction);
        return Ok(*key);
    }
    if keys.len() >= MAX_SORT_KEYS {
        return Err(Error::new(codes::TABLE_SORT_TOO_MANY));
    }
    let key = Sort {
        column,
        direction: SortDirection::Ascending,
    };
    keys.push(key);
    Ok(key)
}

/// Drops the keys whose column `exists` does not report, keeping the order of the others.
pub fn retain_sort_keys(keys: &mut Vec<Sort>, exists: impl Fn(usize) -> bool) {
    keys.retain(|key| exists(key.column));
}

/// Sorts the row indices in `rows` by `keys`, the first key first. `compare(a, b, column)` gives
/// the ascending order of rows `a` and `b` in `column`; a descending key reverses it. The sort
/// is stable: rows equal under every key keep the order they have in `rows`. Only indices move;
/// the cell data stays where the source keeps it.
pub fn sort_rows(
    rows: &mut [usize],
    keys: &[Sort],
    mut compare: impl FnMut(usize, usize, usize) -> Ordering,
) {
    rows.sort_by(|&a, &b| {
        for key in keys {
            let ordering = compare(a, b, key.column);
            let ordering = match key.direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
}

fn check_column(columns: usize, column: usize) -> Result<()> {
    if column < columns {
        Ok(())
    } else {
        Err(Error::new(codes::TABLE_COLUMN_OUT_OF_RANGE))
    }
}

fn reversed(direction: SortDirection) -> SortDirection {
    match direction {
        SortDirection::Ascending => SortDirection::Descending,
        SortDirection::Descending => SortDirection::Ascending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_permutation(order: &[usize], columns: usize) -> bool {
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        sorted == (0..columns).collect::<Vec<_>>()
    }

    fn key(column: usize, direction: SortDirection) -> Sort {
        Sort { column, direction }
    }

    fn code_of(result: Result<()>) -> &'static str {
        result.unwrap_err().code()
    }

    #[test]
    fn moves_shift_the_entries_between_the_two_positions() {
        let mut order = vec![0, 1, 2, 3];
        move_column(&mut order, 0, 2).unwrap();
        assert_eq!(order, vec![1, 2, 0, 3]);
        move_column(&mut order, 3, 0).unwrap();
        assert_eq!(order, vec![3, 1, 2, 0]);
        assert!(is_permutation(&order, 4));
    }

    #[test]
    fn moving_to_the_same_position_changes_nothing() {
        let mut order = vec![2, 0, 1];
        move_column(&mut order, 1, 1).unwrap();
        assert_eq!(order, vec![2, 0, 1]);
    }

    #[test]
    fn out_of_range_moves_are_refused_and_leave_the_order_alone() {
        let mut order = vec![0, 1, 2];
        assert_eq!(
            code_of(move_column(&mut order, 3, 0)),
            "ui.table.column_out_of_range"
        );
        assert_eq!(
            code_of(move_column(&mut order, 0, 3)),
            "ui.table.column_out_of_range"
        );
        assert_eq!(order, vec![0, 1, 2]);
        let mut empty: Vec<usize> = Vec::new();
        assert!(move_column(&mut empty, 0, 0).is_err());
    }

    #[test]
    fn many_moves_keep_a_permutation() {
        let mut order: Vec<usize> = (0..12).collect();
        for step in 0..200_usize {
            let from = (step * 7) % 12;
            let to = (step * 5 + 3) % 12;
            move_column(&mut order, from, to).unwrap();
            assert!(is_permutation(&order, 12));
        }
    }

    #[test]
    fn reconciling_keeps_known_columns_in_order_and_appends_new_ones() {
        assert_eq!(reconcile_order(&[2, 0, 1], 3), vec![2, 0, 1]);
        // Index 4 no longer exists and index 2 is new: index 2 follows the known ones.
        assert_eq!(reconcile_order(&[4, 1, 0, 3], 4), vec![1, 0, 3, 2]);
        // A duplicate is dropped; a fresh table takes the identity.
        assert_eq!(reconcile_order(&[1, 1, 0], 2), vec![1, 0]);
        assert_eq!(reconcile_order(&[], 3), vec![0, 1, 2]);
        assert!(is_permutation(&reconcile_order(&[9, 9, 2], 5), 5));
    }

    #[test]
    fn a_plain_click_sets_one_key_and_toggles_its_direction() {
        let mut keys = Vec::new();
        let first = sort_single(&mut keys, 3, 1).unwrap();
        assert_eq!(first, key(1, SortDirection::Ascending));
        let again = sort_single(&mut keys, 3, 1).unwrap();
        assert_eq!(again, key(1, SortDirection::Descending));
        assert_eq!(keys, vec![key(1, SortDirection::Descending)]);
        // A plain click elsewhere replaces every key.
        sort_additive(&mut keys, 3, 0).unwrap();
        sort_single(&mut keys, 3, 2).unwrap();
        assert_eq!(keys, vec![key(2, SortDirection::Ascending)]);
    }

    #[test]
    fn a_modified_click_adds_then_toggles_keys_in_place() {
        let mut keys = Vec::new();
        sort_single(&mut keys, 4, 2).unwrap();
        sort_additive(&mut keys, 4, 0).unwrap();
        sort_additive(&mut keys, 4, 3).unwrap();
        assert_eq!(
            keys,
            vec![
                key(2, SortDirection::Ascending),
                key(0, SortDirection::Ascending),
                key(3, SortDirection::Ascending),
            ]
        );
        // Toggling the first key keeps its place.
        let toggled = sort_additive(&mut keys, 4, 2).unwrap();
        assert_eq!(toggled, key(2, SortDirection::Descending));
        assert_eq!(keys[0], key(2, SortDirection::Descending));
    }

    #[test]
    fn the_key_limit_refuses_a_new_key_but_still_toggles_existing_ones() {
        let mut keys = Vec::new();
        for column in 0..MAX_SORT_KEYS {
            sort_additive(&mut keys, 8, column).unwrap();
        }
        assert_eq!(keys.len(), MAX_SORT_KEYS);
        let refused = sort_additive(&mut keys, 8, 5).unwrap_err();
        assert_eq!(refused.code(), "ui.table.sort_too_many");
        assert_eq!(keys.len(), MAX_SORT_KEYS, "a refused key changes nothing");
        assert!(sort_additive(&mut keys, 8, 1).is_ok());
        assert_eq!(keys[1].direction, SortDirection::Descending);
    }

    #[test]
    fn sort_clicks_refuse_a_column_that_does_not_exist() {
        let mut keys = vec![key(0, SortDirection::Ascending)];
        assert_eq!(
            sort_single(&mut keys, 2, 2).unwrap_err().code(),
            "ui.table.column_out_of_range"
        );
        assert_eq!(
            sort_additive(&mut keys, 2, 7).unwrap_err().code(),
            "ui.table.column_out_of_range"
        );
        assert_eq!(keys, vec![key(0, SortDirection::Ascending)]);
    }

    #[test]
    fn keys_of_vanished_columns_are_dropped_and_the_rest_keep_their_order() {
        let mut keys = vec![
            key(3, SortDirection::Descending),
            key(1, SortDirection::Ascending),
            key(0, SortDirection::Ascending),
        ];
        retain_sort_keys(&mut keys, |column| column != 3);
        assert_eq!(
            keys,
            vec![
                key(1, SortDirection::Ascending),
                key(0, SortDirection::Ascending)
            ]
        );
    }

    /// Rows with the value of each column given as a table of `values[row][column]`.
    fn by_values(values: &[[i32; 3]]) -> impl Fn(usize, usize, usize) -> Ordering + '_ {
        move |a, b, column| {
            let x = values.get(a).and_then(|r| r.get(column)).copied();
            let y = values.get(b).and_then(|r| r.get(column)).copied();
            x.cmp(&y)
        }
    }

    #[test]
    fn a_sort_is_stable_for_ties_and_keeps_only_indices() {
        let values = [[1, 0, 0], [0, 0, 0], [1, 0, 0], [0, 0, 0]];
        let mut rows = vec![0, 1, 2, 3];
        sort_rows(
            &mut rows,
            &[key(0, SortDirection::Ascending)],
            by_values(&values),
        );
        assert_eq!(rows, vec![1, 3, 0, 2]);
    }

    #[test]
    fn three_keys_apply_in_order_with_directions() {
        // Columns: 0 is the first key, 1 the second, 2 the third.
        let values = [[2, 1, 9], [1, 5, 9], [2, 5, 1], [1, 5, 2], [2, 1, 0]];
        let mut rows: Vec<usize> = (0..values.len()).collect();
        sort_rows(
            &mut rows,
            &[
                key(0, SortDirection::Ascending),
                key(1, SortDirection::Descending),
                key(2, SortDirection::Ascending),
            ],
            by_values(&values),
        );
        // Column 0 ascending gives the 1s (rows 1, 3) before the 2s (rows 0, 2, 4). Among the 1s,
        // column 1 ties at 5 and column 2 ascending puts row 3 (2) before row 1 (9). Among the 2s,
        // column 1 descending puts row 2 (5) first; rows 4 and 0 tie at 1 and column 2 puts row 4
        // (0) before row 0 (9).
        assert_eq!(rows, vec![3, 1, 2, 4, 0]);
    }

    #[test]
    fn repeated_sorts_of_the_same_input_agree() {
        let values = [[3, 0, 0], [1, 0, 0], [3, 0, 0], [2, 0, 0]];
        let keys = [key(0, SortDirection::Descending)];
        let mut first: Vec<usize> = (0..4).collect();
        let mut second = first.clone();
        sort_rows(&mut first, &keys, by_values(&values));
        sort_rows(&mut second, &keys, by_values(&values));
        assert_eq!(first, second);
        assert_eq!(first, vec![0, 2, 3, 1]);
    }

    #[test]
    fn an_empty_key_list_leaves_the_rows_in_place() {
        let values = [[2, 0, 0], [1, 0, 0]];
        let mut rows = vec![0, 1];
        sort_rows(&mut rows, &[], by_values(&values));
        assert_eq!(rows, vec![0, 1]);
    }
}
