use crate::*;

impl Contract {
    pub(crate) fn ft_transfer(
        &self,
        token_account_id: AccountId,
        receiver: AccountId,
        amount: Balance,
    ) -> Promise {
        if amount == 0 {
            // NEP-141 forbids zero token transfers
            //
            // Return empty promise
            return Promise::new(receiver);
        }

        if Contract::is_aurora_address(&receiver) {
            if token_account_id == Contract::aurora_account_id() {
                return ext_fungible_token::ft_transfer_call(
                    Contract::aurora_account_id(),
                    U128(amount),
                    None,
                    Contract::aurora_transfer_call_msg(receiver),
                    Contract::aurora_account_id(),
                    ONE_YOCTO,
                    env::prepaid_gas() - env::used_gas() - Gas::ONE_TERA * 10,
                );
            } else {
                return ext_fungible_token::ft_transfer_call(
                    Contract::aurora_account_id(),
                    U128(amount),
                    None,
                    receiver.to_string(),
                    token_account_id,
                    ONE_YOCTO,
                    env::prepaid_gas() - env::used_gas() - Gas::ONE_TERA * 10,
                );
            }
        } else {
            ext_fungible_token::ft_transfer(
                receiver,
                U128(amount),
                None,
                token_account_id,
                ONE_YOCTO,
                env::prepaid_gas() - env::used_gas() - Gas::ONE_TERA * 10,
            )
        }
    }
}
