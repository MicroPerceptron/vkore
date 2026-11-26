//! Merging the commands that arrive together.
//!
//! The desktop sends a pointer move for every mouse event and a wheel delta for
//! every notch, and the helper used to run its event loop once per command. A
//! burst of them is a burst of work whose only visible result is the last move
//! and the sum of the wheel. Merge neighbours that mean one thing; never reorder
//! across a command that means something else (a press between two moves keeps
//! both moves).

use vkore_servo_wire::{Command, Pointer};

/// The most commands taken at once, so a flood cannot starve the paint that
/// follows it.
pub const MAX_BATCH: usize = 256;

pub fn coalesce(commands: Vec<Command>) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::with_capacity(commands.len());
    for command in commands {
        match (out.last_mut(), &command) {
            (
                Some(Command::Pointer(a, _, _, Pointer::Move)),
                Command::Pointer(b, _, _, Pointer::Move),
            ) if a == b => *out.last_mut().unwrap() = command,
            (
                Some(Command::Pointer(a, _, _, Pointer::Wheel(before))),
                Command::Pointer(b, x, y, Pointer::Wheel(now)),
            ) if a == b => {
                let sum = before.saturating_add(*now);
                *out.last_mut().unwrap() = Command::Pointer(*b, *x, *y, Pointer::Wheel(sum));
            }
            (Some(Command::Resize(a, ..)), Command::Resize(b, ..)) if a == b => {
                *out.last_mut().unwrap() = command
            }
            (Some(Command::Paint(a)), Command::Paint(b)) if a == b => {}
            _ => out.push(command),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(id: u64, x: u32) -> Command {
        Command::Pointer(id, x, x, Pointer::Move)
    }

    #[test]
    fn a_run_of_moves_keeps_only_the_last() {
        let out = coalesce((0..500).map(|n| moves(1, n)).collect());
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Command::Pointer(1, 499, 499, Pointer::Move)
        ));
    }

    #[test]
    fn a_press_between_moves_keeps_both_moves() {
        let out = coalesce(vec![
            moves(1, 1),
            moves(1, 2),
            Command::Pointer(1, 2, 2, Pointer::Down),
            moves(1, 3),
            moves(1, 4),
            Command::Pointer(1, 4, 4, Pointer::Up),
        ]);
        let shapes: Vec<_> = out
            .iter()
            .map(|c| match c {
                Command::Pointer(_, x, _, p) => (*x, std::mem::discriminant(p)),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(shapes.len(), 4);
        assert_eq!(shapes[0].0, 2);
        assert_eq!(shapes[2].0, 4);
    }

    #[test]
    fn moves_on_different_tabs_are_not_merged() {
        assert_eq!(
            coalesce(vec![moves(1, 1), moves(2, 2), moves(1, 3)]).len(),
            3
        );
    }

    #[test]
    fn wheel_deltas_add_up_at_the_latest_position() {
        let out = coalesce(vec![
            Command::Pointer(1, 5, 5, Pointer::Wheel(40)),
            Command::Pointer(1, 6, 7, Pointer::Wheel(-10)),
            Command::Pointer(1, 8, 9, Pointer::Wheel(25)),
        ]);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Command::Pointer(1, 8, 9, Pointer::Wheel(55))
        ));
    }

    #[test]
    fn a_wheel_sum_saturates_instead_of_wrapping() {
        let out = coalesce(vec![
            Command::Pointer(1, 0, 0, Pointer::Wheel(i32::MAX)),
            Command::Pointer(1, 0, 0, Pointer::Wheel(i32::MAX)),
        ]);
        assert!(matches!(
            out[0],
            Command::Pointer(_, _, _, Pointer::Wheel(i32::MAX))
        ));
    }

    #[test]
    fn only_the_last_resize_and_one_paint_survive() {
        let out = coalesce(vec![
            Command::Resize(1, 100, 100, 1.),
            Command::Resize(1, 200, 150, 2.),
            Command::Paint(1),
            Command::Paint(1),
        ]);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Command::Resize(1, 200, 150, _)));
        assert!(matches!(out[1], Command::Paint(1)));
    }

    #[test]
    fn order_across_other_commands_is_never_changed() {
        let out = coalesce(vec![
            Command::Resize(1, 10, 10, 1.),
            Command::Navigate(1, "https://example.org".into()),
            Command::Resize(1, 20, 20, 1.),
            Command::Paint(1),
            Command::Back(1),
            Command::Paint(1),
        ]);
        assert_eq!(out.len(), 6);
    }
}
