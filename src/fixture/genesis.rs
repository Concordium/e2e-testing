//! Genesis fixture — programmatic genesis block generation.
//!
//! Builds fresh private chain genesis blocks entirely in memory using
//! `concordium_rust_sdk::genesis`.

use anyhow::{Context, Result};
use concordium_rust_sdk::{
    common::types::{Amount, Ratio, Timestamp},
    genesis::{
        self, CoreGenesisParametersV1, GenesisChainParametersV3, ProtocolParamsCPV3,
        RewardParametersCPV2,
        builder::{
            FreshAccountConfig, GovernanceKeyLevelConfig, GovernanceKeySpec,
            GovernanceKeysGenerateConfig, Level2AccessConfig, Level2GovernanceKeysConfig,
        },
        output::GeneratedKeyPair,
    },
    id::types::{ArIdentity, IpIdentity, SignatureThreshold},
    smart_contracts::common::Duration,
    types::{
        AmountFraction, BakerCredentials, CapitalBound, CommissionRanges, CooldownParameters,
        DurationSeconds, Energy, Epoch, ExchangeRate, FinalizationCommitteeParameters,
        GASRewardsV1, InclusiveRange, LeverageFactor, MintDistributionV1, MintRate,
        PartsPerHundredThousands, PoolParameters, RewardPeriodLength, TimeParameters,
        TimeoutParameters, TransactionFeeDistribution, UpdateKeysIndex, UpdateKeysThreshold,
        ValidatorScoreParameters, hashes::LeadershipElectionNonce,
    },
};

// ── Public types ──────────────────────────────────────────────────────────────

/// Account signing data with an asynchronous lock.
/// Hold the lock from nonce selection through transaction finalization.
pub type LockedAccount = tokio::sync::Mutex<concordium_rust_sdk::genesis::types::GenesisAccount>;

/// Validator signing keys with a synchronous lock.
/// Release the lock before an asynchronous call.
pub type LockedValidator = std::sync::Mutex<BakerCredentials>;

/// Genesis state and signing keys for a temporary chain.
pub struct GenesisFixture {
    /// Genesis state to encode in the node's `genesis.dat` file.
    pub genesis_data: genesis::GenesisData,
    /// Credentials in genesis validator order.
    pub validators: Vec<LockedValidator>,
    /// Accounts in genesis order, including validators, with signing keys.
    pub accounts: Vec<LockedAccount>,
    /// Private level-2 keys for chain updates. Do not log these keys.
    pub governance_keys: Vec<GeneratedKeyPair>,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl GenesisFixture {
    /// Generate a fresh P10 single-validator genesis block timestamped to now.
    ///
    /// Creates:
    /// - 1 validator account (staking 3 000 000 CCD)
    /// - 1 funded regular account (100 000 CCD)
    /// - 1 identity provider and 1 anonymity revoker
    /// - Governance keys with 2-of-3 thresholds
    pub fn single_validator_p10() -> Result<Self> {
        Self::build_p10(1, 1, 3_600_000)
    }

    /// Generate a fresh P10 four-validator genesis block timestamped to now.
    ///
    /// Creates:
    /// - 4 validator accounts (staking 3 000 000 CCD each)
    /// - 2 funded regular accounts (100 000 CCD each)
    /// - 1 identity provider and 1 anonymity revoker
    /// - Governance keys with 2-of-3 thresholds
    /// - 30-second epochs for protocol-update activation
    #[cfg(test)]
    pub fn four_validator_p10() -> Result<Self> {
        Self::build_p10(4, 2, 30_000)
    }

    /// Generate four P10 validators and the regular accounts requested by protocol tests.
    pub fn four_validator_p10_with_accounts(regular_account_count: u32) -> Result<Self> {
        Self::build_p10(4, regular_account_count, 30_000)
    }

    /// Build P10 genesis data with equal validator stakes. Durations use milliseconds.
    /// The validator count must be at least one.
    fn build_p10(
        validator_count: u32,
        regular_account_count: u32,
        epoch_duration_ms: u64,
    ) -> Result<Self> {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;

        let builder = genesis::genesis_builder_p10()
            .generate_crypto_params("concordium-e2e-test".to_string())
            .generate_identity_providers(IpIdentity::from(0), 1)
            .generate_anonymity_revokers(
                ArIdentity::try_from(1u32).map_err(|e| anyhow::anyhow!(e))?,
                1,
            )
            // Foundation / validator account (staked)
            .generate_accounts(FreshAccountConfig {
                count: 1,
                balance: Amount::from_micro_ccd(3_500_000_000_000u64),
                stake: Some(Amount::from_micro_ccd(3_000_000_000_000u64)),
                num_keys: 1,
                threshold: SignatureThreshold::ONE,
                identity_provider: IpIdentity::from(0),
                restake_earnings: true,
                foundation: true,
            });
        // Only the first validator is the foundation account. The builder
        // rejects empty batches, so leave the smoke topology unchanged.
        let builder = if validator_count > 1 {
            builder.generate_accounts(FreshAccountConfig {
                count: validator_count - 1,
                balance: Amount::from_micro_ccd(3_500_000_000_000u64),
                stake: Some(Amount::from_micro_ccd(3_000_000_000_000u64)),
                num_keys: 1,
                threshold: SignatureThreshold::ONE,
                identity_provider: IpIdentity::from(0),
                restake_earnings: true,
                foundation: false,
            })
        } else {
            builder
        };
        let builder = if regular_account_count > 0 {
            builder.generate_accounts(FreshAccountConfig {
                count: regular_account_count,
                balance: Amount::from_micro_ccd(100_000_000_000u64),
                stake: None,
                num_keys: 1,
                threshold: SignatureThreshold::ONE,
                identity_provider: IpIdentity::from(0),
                restake_earnings: false,
                foundation: false,
            })
        } else {
            builder
        };
        let output = builder
            .generate_governance_keys(make_governance_keys())
            .with_protocol(make_protocol_params(
                now_ms,
                validator_count,
                epoch_duration_ms,
            ))
            .build()
            .context("Failed to build P10 genesis block")?;

        let validators = output
            .baker_credentials
            .into_iter()
            .map(std::sync::Mutex::new)
            .collect();

        let accounts = output
            .account_data
            .into_iter()
            .map(tokio::sync::Mutex::new)
            .collect();

        Ok(Self {
            genesis_data: output.genesis_data,
            validators,
            accounts,
            governance_keys: output.generated_level2_key_pairs,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Select authorized key indices. The threshold must be nonzero and must not exceed the key count.
fn make_access(keys: &[u16], threshold: u16) -> Level2AccessConfig {
    Level2AccessConfig {
        authorized_keys: keys.iter().map(|&i| UpdateKeysIndex { index: i }).collect(),
        threshold: UpdateKeysThreshold::try_from(threshold).unwrap(),
    }
}

/// Generate three keys per level and require two signatures for each authorized update.
fn make_governance_keys() -> GovernanceKeysGenerateConfig {
    let k: &[u16] = &[0, 1, 2];
    GovernanceKeysGenerateConfig {
        root: GovernanceKeyLevelConfig {
            threshold: UpdateKeysThreshold::try_from(2u16).unwrap(),
            keys: vec![GovernanceKeySpec::Fresh { count: 3 }],
        },
        level1: GovernanceKeyLevelConfig {
            threshold: UpdateKeysThreshold::try_from(2u16).unwrap(),
            keys: vec![GovernanceKeySpec::Fresh { count: 3 }],
        },
        level2: Level2GovernanceKeysConfig {
            keys: vec![GovernanceKeySpec::Fresh { count: 3 }],
            emergency: make_access(k, 2),
            protocol: make_access(k, 2),
            election_difficulty: make_access(k, 2),
            euro_per_energy: make_access(k, 2),
            micro_ccd_per_euro: make_access(k, 2),
            foundation_account: make_access(k, 2),
            mint_distribution: make_access(k, 2),
            transaction_fee_distribution: make_access(k, 2),
            gas_rewards: make_access(k, 2),
            pool_parameters: make_access(k, 2),
            add_anonymity_revoker: make_access(k, 2),
            add_identity_provider: make_access(k, 2),
            cooldown_parameters: make_access(k, 2),
            time_parameters: make_access(k, 2),
            create_plt: Some(make_access(k, 2)),
            token_parameters: None,
        },
    }
}

/// Set test-chain timing and rewards. Include all validators in the finalization committee.
fn make_protocol_params(
    genesis_time_ms: u64,
    validator_count: u32,
    epoch_duration_ms: u64,
) -> ProtocolParamsCPV3 {
    ProtocolParamsCPV3 {
        core: CoreGenesisParametersV1 {
            genesis_time: Timestamp {
                millis: genesis_time_ms,
            },
            epoch_duration: Duration::from_millis(epoch_duration_ms),
            signature_threshold: Ratio::new(2, 3).unwrap(),
        },
        chain: GenesisChainParametersV3 {
            timeout_parameters: TimeoutParameters::new(
                Duration::from_millis(2_000),
                Ratio::new(6, 5).unwrap(), // increase ×1.2
                Ratio::new(4, 5).unwrap(), // decrease ×0.8
            )
            .unwrap(),
            min_block_time: Duration::from_millis(2_000),
            block_energy_limit: Energy { energy: 3_000_000 },
            // 0.00002 EUR per NRG  ⟹  numerator=1, denominator=50_000
            euro_per_energy: ExchangeRate::new_unchecked(1, 50_000),
            // 500 000 µCCD per EUR ⟹  numerator=500_000, denominator=1
            micro_ccd_per_euro: ExchangeRate::new_unchecked(500_000, 1),
            account_creation_limit: 10u16.into(),
            reward_parameters: RewardParametersCPV2 {
                mint_distribution: MintDistributionV1 {
                    baking_reward: AmountFraction::new_unchecked(85_000), // 85%
                    finalization_reward: AmountFraction::new_unchecked(5_000), // 5%
                },
                transaction_fee_distribution: TransactionFeeDistribution {
                    baker: AmountFraction::new_unchecked(45_000), // 45%
                    gas_account: AmountFraction::new_unchecked(45_000), // 45%
                },
                gas_rewards: GASRewardsV1 {
                    baker: AmountFraction::new_unchecked(25_000), // 25%
                    account_creation: AmountFraction::new_unchecked(2_000), // 2%
                    chain_update: AmountFraction::new_unchecked(500), // 0.5%
                },
            },
            time_parameters: TimeParameters {
                // 4 epochs per reward period
                reward_period_length: RewardPeriodLength::from(Epoch { epoch: 4 }),
                // ≈2.61157877e-4 mint rate: mantissa=261157877, exponent=12
                mint_per_payday: MintRate {
                    mantissa: 261_157_877,
                    exponent: 12,
                },
            },
            pool_parameters: PoolParameters {
                passive_finalization_commission: AmountFraction::new_unchecked(100_000), // 100%
                passive_baking_commission: AmountFraction::new_unchecked(10_000),        // 10%
                passive_transaction_commission: AmountFraction::new_unchecked(10_000),   // 10%
                commission_bounds: CommissionRanges {
                    finalization: InclusiveRange {
                        min: AmountFraction::new_unchecked(50_000),
                        max: AmountFraction::new_unchecked(100_000),
                    },
                    baking: InclusiveRange {
                        min: AmountFraction::new_unchecked(5_000),
                        max: AmountFraction::new_unchecked(10_000),
                    },
                    transaction: InclusiveRange {
                        min: AmountFraction::new_unchecked(5_000),
                        max: AmountFraction::new_unchecked(20_000),
                    },
                },
                minimum_equity_capital: Amount::from_micro_ccd(500_000_000_000u64),
                capital_bound: CapitalBound {
                    bound: AmountFraction::new_unchecked(10_000), // 10%
                },
                leverage_bound: LeverageFactor::new_integral(3),
            },
            cooldown_parameters: CooldownParameters {
                pool_owner_cooldown: DurationSeconds { seconds: 800 },
                delegator_cooldown: DurationSeconds { seconds: 1_000 },
            },
            finalization_committee_parameters: FinalizationCommitteeParameters {
                min_finalizers: validator_count,
                max_finalizers: validator_count,
                finalizers_relative_stake_threshold: PartsPerHundredThousands::new_unchecked(0),
            },
            validator_score_parameters: ValidatorScoreParameters {
                max_missed_rounds: 10,
            },
            max_lock_duration: None,
        },
        leadership_election_nonce: LeadershipElectionNonce::from([0u8; 32]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_genesis_preserves_existing_topology() -> Result<()> {
        let fixture = GenesisFixture::single_validator_p10()?;
        assert_eq!(fixture.validators.len(), 1);
        assert_eq!(fixture.accounts.len(), 2);
        assert_eq!(fixture.governance_keys.len(), 3);
        let genesis::GenesisData::P10 { core, .. } = fixture.genesis_data else {
            panic!("expected P10 genesis");
        };
        assert_eq!(core.epoch_duration, Duration::from_millis(3_600_000));
        Ok(())
    }

    #[test]
    fn protocol_account_budget_preserves_validator_configuration() -> Result<()> {
        for count in [0, 7] {
            let fixture = GenesisFixture::four_validator_p10_with_accounts(count)?;
            assert_eq!(fixture.validators.len(), 4);
            assert_eq!(fixture.accounts.len(), 4 + count as usize);
            let genesis::GenesisData::P10 {
                core,
                initial_state,
            } = fixture.genesis_data
            else {
                panic!("expected P10 genesis")
            };
            assert_eq!(core.signature_threshold, Ratio::new(2, 3).unwrap());
            for account in &initial_state.accounts[..4] {
                let baker = account.baker.as_ref().unwrap();
                assert_eq!(baker.stake, Amount::from_micro_ccd(3_000_000_000_000));
                assert!(baker.restake_earnings);
            }
            for account in &initial_state.accounts[4..] {
                assert!(account.baker.is_none());
                assert_eq!(account.balance, Amount::from_micro_ccd(100_000_000_000));
            }
        }
        Ok(())
    }

    #[test]
    fn upgrade_genesis_has_equal_stake_and_short_epochs() -> Result<()> {
        let fixture = GenesisFixture::four_validator_p10()?;
        assert_eq!(fixture.validators.len(), 4);
        assert_eq!(fixture.accounts.len(), 6);
        assert_eq!(fixture.governance_keys.len(), 3);
        let genesis::GenesisData::P10 {
            core,
            initial_state,
        } = fixture.genesis_data
        else {
            panic!("expected P10 genesis");
        };
        assert_eq!(core.signature_threshold, Ratio::new(2, 3).unwrap());
        assert_eq!(core.epoch_duration, Duration::from_millis(30_000));
        let stakes: Vec<_> = initial_state
            .accounts
            .iter()
            .filter_map(|account| account.baker.as_ref().map(|baker| baker.stake))
            .collect();
        assert_eq!(stakes, vec![Amount::from_micro_ccd(3_000_000_000_000); 4]);
        let params = make_protocol_params(0, 4, 30_000);
        assert_eq!(
            params
                .chain
                .finalization_committee_parameters
                .min_finalizers,
            4
        );
        assert_eq!(
            params
                .chain
                .finalization_committee_parameters
                .max_finalizers,
            4
        );
        Ok(())
    }
}
