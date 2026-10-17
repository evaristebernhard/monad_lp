pub use crate::grpc::types::{
    AccountFilter, AccountFilterData, AccountFilterMemcmp, EventTypeFilter, TransactionFilter,
};

use crate::grpc::program_ids::get_program_ids_for_protocols;
use crate::grpc::types::Protocol;

impl TransactionFilter {
    pub fn for_protocols(protocols: &[Protocol]) -> Self {
        let program_ids = get_program_ids_for_protocols(protocols);
        Self {
            account_include: program_ids,
            account_exclude: Vec::new(),
            account_required: Vec::new(),
        }
    }
}

impl AccountFilter {
    pub fn for_protocols(protocols: &[Protocol]) -> Self {
        let program_ids = get_program_ids_for_protocols(protocols);
        Self { account: Vec::new(), owner: program_ids, filters: Vec::new() }
    }
}
