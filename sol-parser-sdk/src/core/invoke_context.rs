use std::collections::HashMap;

use smallvec::SmallVec;
use solana_sdk::pubkey::Pubkey;

pub(crate) type InvokePosition = (i32, i32);

pub(crate) trait InvokeLookup {
    fn get_invokes(&self, program_id: &Pubkey) -> Option<&[InvokePosition]>;
}

impl InvokeLookup for HashMap<Pubkey, Vec<InvokePosition>> {
    #[inline]
    fn get_invokes(&self, program_id: &Pubkey) -> Option<&[InvokePosition]> {
        self.get(program_id).map(Vec::as_slice)
    }
}

/// Per-transaction invoke positions. Real swaps usually touch one or two supported
/// programs, so keeping both the program table and common position lists inline
/// avoids hashing and heap allocation on the parser hot path.
#[derive(Default)]
pub(crate) struct InvokeContext {
    entries: SmallVec<[(Pubkey, SmallVec<[InvokePosition; 4]>); 2]>,
}

impl InvokeContext {
    #[inline]
    pub(crate) fn push(&mut self, program_id: Pubkey, position: InvokePosition) {
        if let Some((_, positions)) =
            self.entries.iter_mut().find(|(candidate, _)| *candidate == program_id)
        {
            positions.push(position);
            return;
        }

        let mut positions = SmallVec::new();
        positions.push(position);
        self.entries.push((program_id, positions));
    }
}

impl InvokeLookup for InvokeContext {
    #[inline]
    fn get_invokes(&self, program_id: &Pubkey) -> Option<&[InvokePosition]> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == program_id)
            .map(|(_, positions)| positions.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_positions_by_program() {
        let first = Pubkey::new_unique();
        let second = Pubkey::new_unique();
        let mut context = InvokeContext::default();
        context.push(first, (1, -1));
        context.push(second, (2, 0));
        context.push(first, (1, 3));

        assert_eq!(context.get_invokes(&first), Some(&[(1, -1), (1, 3)][..]));
        assert_eq!(context.get_invokes(&second), Some(&[(2, 0)][..]));
        assert_eq!(context.get_invokes(&Pubkey::new_unique()), None);
    }

    #[test]
    fn spills_without_losing_programs_or_positions() {
        let programs = [Pubkey::new_unique(), Pubkey::new_unique(), Pubkey::new_unique()];
        let mut context = InvokeContext::default();
        for outer in 0..6 {
            context.push(programs[0], (outer, -1));
        }
        context.push(programs[1], (10, 1));
        context.push(programs[2], (11, 2));

        assert_eq!(context.get_invokes(&programs[0]).map(<[_]>::len), Some(6));
        assert_eq!(context.get_invokes(&programs[1]), Some(&[(10, 1)][..]));
        assert_eq!(context.get_invokes(&programs[2]), Some(&[(11, 2)][..]));
    }
}
