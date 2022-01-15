use crate::*;

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[serde(crate = "near_sdk::serde")]
pub struct Stream {
    #[serde(with = "b58_dec_format")]
    pub id: CryptoHash,
    pub description: Option<String>,
    pub owner_id: AccountId,
    pub receiver_id: AccountId,
    pub token_account_id: AccountId,

    #[serde(with = "u64_dec_format")]
    pub timestamp_created: Timestamp,
    #[serde(with = "u64_dec_format")]
    pub last_action: Timestamp,

    #[serde(with = "u128_dec_format")]
    pub balance: Balance,
    #[serde(with = "u128_dec_format")]
    pub tokens_per_sec: Balance,

    pub status: StreamStatus,
    #[serde(with = "u128_dec_format")]
    pub tokens_total_withdrawn: Balance,

    pub is_expirable: bool,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub enum VStream {
    Current(Stream),
}

impl From<VStream> for Stream {
    fn from(v: VStream) -> Self {
        match v {
            VStream::Current(c) => c,
        }
    }
}

impl From<Stream> for VStream {
    fn from(c: Stream) -> Self {
        VStream::Current(c)
    }
}

impl Stream {
    pub(crate) fn new(
        description: Option<String>,
        owner_id: AccountId,
        receiver_id: AccountId,
        token_account_id: AccountId,
        initial_balance: Balance,
        tokens_per_sec: Balance,
        is_expirable: bool,
    ) -> Stream {
        let id = env::sha256(&env::random_seed())
            .as_slice()
            .try_into()
            .unwrap();
        Self {
            id,
            description,
            owner_id,
            receiver_id,
            token_account_id,
            timestamp_created: env::block_timestamp(),
            last_action: env::block_timestamp(),
            balance: initial_balance,
            tokens_per_sec,
            status: StreamStatus::Initialized,
            tokens_total_withdrawn: 0,
            is_expirable,
        }
    }

    pub(crate) fn process_withdraw(&mut self, token: &Token) -> (Balance, Balance) {
        let gross_payment = self.available_to_withdraw();
        self.tokens_total_withdrawn += gross_payment;
        let (payment, commission) =
            token.apply_commission(std::cmp::min(gross_payment, self.balance));
        if self.balance > gross_payment {
            self.balance -= gross_payment;
        } else {
            self.balance = 0;
            self.status = StreamStatus::Finished {
                reason: StreamFinishReason::FinishedNatually,
            };
        }
        // This update of last_action is useless here
        // however it helps to keep invariant of stream status.
        self.last_action = env::block_timestamp();
        (payment, commission)
    }

    pub(crate) fn available_to_withdraw(&self) -> Balance {
        if self.status == StreamStatus::Active {
            let period = (env::block_timestamp() - self.last_action) as u128;
            std::cmp::min(
                self.balance,
                period / TICKS_PER_SECOND * self.tokens_per_sec,
            )
        } else {
            0
        }
    }
}

impl Contract {
    pub(crate) fn process_action(
        &mut self,
        stream: &mut Stream,
        action_type: ActionType,
    ) -> Result<Vec<Promise>, ContractError> {
        let mut owner = self.extract_account_or_create(&stream.owner_id);
        let mut receiver = self.extract_account_or_create(&stream.receiver_id);
        let mut promises = vec![];

        if action_type == ActionType::Init {
            debug_assert!(owner.inactive_streams.insert(&stream.id));
            debug_assert!(receiver.inactive_streams.insert(&stream.id));
        } else {
            debug_assert!(!stream.status.is_terminated());
            match action_type {
                ActionType::Start => {
                    debug_assert!(owner.inactive_streams.remove(&stream.id));
                    debug_assert!(receiver.inactive_streams.remove(&stream.id));
                    debug_assert!(owner.active_streams.insert(&stream.id));
                    debug_assert!(receiver.active_streams.insert(&stream.id));
                    *owner
                        .total_outgoing
                        .entry(stream.token_account_id.clone())
                        .or_insert(0) += stream.tokens_per_sec;
                    *receiver
                        .total_incoming
                        .entry(stream.token_account_id.clone())
                        .or_insert(0) += stream.tokens_per_sec;
                    stream.status = StreamStatus::Active;
                }
                ActionType::Pause => {
                    debug_assert_eq!(stream.status, StreamStatus::Active);
                    promises.push(self.process_payment(stream, &mut receiver)?);
                    debug_assert!(owner.active_streams.remove(&stream.id));
                    debug_assert!(receiver.active_streams.remove(&stream.id));
                    debug_assert!(owner.inactive_streams.insert(&stream.id));
                    debug_assert!(receiver.inactive_streams.insert(&stream.id));
                    *owner
                        .total_outgoing
                        .get_mut(&stream.token_account_id)
                        .unwrap() -= stream.tokens_per_sec;
                    *receiver
                        .total_incoming
                        .get_mut(&stream.token_account_id)
                        .unwrap() -= stream.tokens_per_sec;
                    if stream.status == StreamStatus::Active {
                        // The stream may be stopped while payment processing
                        stream.status = StreamStatus::Paused;
                    }
                }
                ActionType::Stop { reason } => {
                    if stream.status == StreamStatus::Active {
                        promises.push(self.process_payment(stream, &mut receiver)?);
                        debug_assert!(owner.active_streams.remove(&stream.id));
                        debug_assert!(receiver.active_streams.remove(&stream.id));
                        debug_assert!(owner.inactive_streams.insert(&stream.id));
                        debug_assert!(receiver.inactive_streams.insert(&stream.id));
                        *owner
                            .total_outgoing
                            .get_mut(&stream.token_account_id)
                            .unwrap() -= stream.tokens_per_sec;
                        *receiver
                            .total_incoming
                            .get_mut(&stream.token_account_id)
                            .unwrap() -= stream.tokens_per_sec;
                    } else {
                        // Can be initialized or paused - nothing to do in this case
                    }
                    if !stream.status.is_terminated() {
                        // Refund can be requested only if stream is not terminated naturally yet
                        promises.push(self.process_refund(stream)?);
                        stream.status = StreamStatus::Finished { reason };
                    }
                }
                ActionType::Init => {
                    // Processed separately
                    unreachable!();
                }
                ActionType::Withdraw => {
                    debug_assert_eq!(stream.status, StreamStatus::Active);
                    promises.push(self.process_payment(stream, &mut receiver)?);
                    if stream.status.is_terminated() {
                        debug_assert_eq!(
                            stream.status,
                            StreamStatus::Finished {
                                reason: StreamFinishReason::FinishedNatually
                            }
                        );
                        debug_assert!(owner.active_streams.remove(&stream.id));
                        debug_assert!(receiver.active_streams.remove(&stream.id));
                        debug_assert!(owner.inactive_streams.insert(&stream.id));
                        debug_assert!(receiver.inactive_streams.insert(&stream.id));
                        *owner
                            .total_outgoing
                            .get_mut(&stream.token_account_id)
                            .unwrap() -= stream.tokens_per_sec;
                        *receiver
                            .total_incoming
                            .get_mut(&stream.token_account_id)
                            .unwrap() -= stream.tokens_per_sec;
                    }
                }
            }
        }

        stream.last_action = env::block_timestamp();
        self.save_account(&stream.owner_id, owner)?;
        self.save_account(&stream.receiver_id, receiver)?;

        Ok(promises)
    }

    pub(crate) fn process_payment(
        &mut self,
        stream: &mut Stream,
        account: &mut Account,
    ) -> Result<Promise, ContractError> {
        let token = self.dao.get_token(&stream.token_account_id)?;
        let (payment, _commission) = stream.process_withdraw(&token);
        *account
            .total_received
            .entry(stream.token_account_id.clone())
            .or_insert(0) += payment;
        // TODO update stats
        self.ft_transfer(&token, &stream.receiver_id, payment, Some(true))
    }

    pub(crate) fn process_refund(&mut self, stream: &mut Stream) -> Result<Promise, ContractError> {
        let token = self.dao.get_token(&stream.token_account_id)?;
        let payment = stream.balance;
        stream.balance = 0;
        // TODO update stats
        self.ft_transfer(&token, &stream.owner_id, payment, Some(true))
    }

    pub(crate) fn extract_stream(&mut self, stream_id: &StreamId) -> Result<Stream, ContractError> {
        match self.streams.remove(&stream_id) {
            Some(vstream) => Ok(vstream.into()),
            None => Err(ContractError::UnreachableStream {
                stream_id: (*stream_id).into(),
            }),
        }
    }

    pub(crate) fn save_stream(
        &mut self,
        stream_id: &StreamId,
        stream: Stream,
    ) -> Result<(), ContractError> {
        if *stream_id != stream.id {
            return Err(ContractError::DataCorruption);
        }
        match self.streams.insert(stream_id, &stream.into()) {
            None => Ok(()),
            Some(_) => Err(ContractError::DataCorruption),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::test_env::{alice, bob};
    use near_sdk::{env, test_utils::VMContextBuilder, testing_env};

    fn new_stream() -> Stream {
        Stream {
            id: env::sha256(&[44, 55, 66]).as_slice().try_into().unwrap(),
            description: Some("blah".to_string()),
            owner_id: alice(),
            receiver_id: bob(),
            token_account_id: "token.near".parse().unwrap(),
            timestamp_created: env::block_timestamp(),
            last_action: env::block_timestamp(),
            balance: 1_000_000_000_000_000_000_000_000_000, // 1e27
            tokens_per_sec: 1_000_000_000_000_000_000_000_000, // 1e24
            status: StreamStatus::Active,
            tokens_total_withdrawn: 0,
            is_expirable: true,
        }
    }

    #[test]
    fn test_available_to_withdraw_all() {
        testing_env!(VMContextBuilder::new()
            .block_timestamp(1633333333000000000)
            .build());
        let stream = new_stream();
        assert_eq!(stream.available_to_withdraw(), 0);
        // 4 months passed
        testing_env!(VMContextBuilder::new()
            .block_timestamp(1643333333000000000)
            .build());
        assert_eq!(stream.available_to_withdraw(), stream.balance);
    }

    #[test]
    fn test_process_withdraw_instant() {
        testing_env!(VMContextBuilder::new()
            .block_timestamp(1633333333000000000)
            .build());
        let mut stream = new_stream();
        let original_balance = stream.balance;
        assert_eq!(stream.available_to_withdraw(), 0);
        let (withdrawn, commission) =
            stream.process_withdraw(&Token::new_unlisted(&"token1.near".parse().unwrap()));
        assert_eq!(withdrawn, 0);
        assert_eq!(commission, 0);
        assert_eq!(stream.available_to_withdraw(), 0);
        assert_eq!(stream.balance, original_balance);
        assert_eq!(stream.tokens_total_withdrawn, 0);
    }

    #[test]
    fn test_process_withdraw_precise() {
        testing_env!(VMContextBuilder::new()
            .block_timestamp(1633333333000000000)
            .build());
        let mut stream = new_stream();
        let original_balance = stream.balance;
        assert_eq!(stream.available_to_withdraw(), 0);
        for i in 1..=1000u128 {
            testing_env!(VMContextBuilder::new()
                .block_timestamp(1633333333000000000 + i as u64 * 1000000000)
                .build());
            assert_eq!(stream.status, StreamStatus::Active);
            assert_eq!(stream.available_to_withdraw(), stream.tokens_per_sec);
            let (withdrawn, commission) =
                stream.process_withdraw(&Token::new_unlisted(&"token1.near".parse().unwrap()));
            assert_eq!(withdrawn, stream.tokens_per_sec);
            assert_eq!(commission, 0);
            assert_eq!(stream.available_to_withdraw(), 0);
            assert_eq!(stream.balance, original_balance - i * stream.tokens_per_sec);
            assert_eq!(stream.tokens_total_withdrawn, i * stream.tokens_per_sec);
        }
        assert_eq!(
            stream.status,
            StreamStatus::Finished {
                reason: StreamFinishReason::FinishedNatually
            }
        );
        // 1 more second
        testing_env!(VMContextBuilder::new()
            .block_timestamp(1643334333000000000)
            .build());
        assert_eq!(stream.available_to_withdraw(), 0);
        let (withdrawn, commission) =
            stream.process_withdraw(&Token::new_unlisted(&"token1.near".parse().unwrap()));
        assert_eq!(withdrawn, 0);
        assert_eq!(commission, 0);
        assert_eq!(stream.available_to_withdraw(), 0);
        assert_eq!(stream.balance, 0);
        assert_eq!(stream.tokens_total_withdrawn, original_balance);
        assert_eq!(
            stream.status,
            StreamStatus::Finished {
                reason: StreamFinishReason::FinishedNatually
            }
        );
    }

    // TODO add tests with commission and fractions
    // TODO test deposit invalid token
    // TODO save stream with listed token
}
