//! Genesis fixture — programmatic genesis block generation.
//!
//! Builds a fresh single-validator private chain genesis block entirely in
//! memory using `concordium_rust_sdk::genesis`. No filesystem access occurs
//! here; the serialised bytes and credentials are returned as in-memory values.

use anyhow::{Context, Result};
use concordium_rust_sdk::{
    common::types::{Amount, Ratio, Timestamp},
    genesis::{
        self,
        builder::{
            FreshAccountConfig, GovernanceKeyLevelConfig, GovernanceKeySpec,
            GovernanceKeysGenerateConfig, Level2AccessConfig, Level2GovernanceKeysConfig,
        },
        CoreGenesisParametersV1, GenesisChainParametersV3, ProtocolParamsCPV3,
        RewardParametersCPV2,
    },
    id::types::{ArIdentity, IpIdentity, SignatureThreshold},
    smart_contracts::common::Duration,
    types::{
        hashes::LeadershipElectionNonce, AmountFraction, BakerCredentials, CapitalBound,
        CommissionRanges, CooldownParameters, DurationSeconds, Energy, Epoch, ExchangeRate,
        FinalizationCommitteeParameters, GASRewardsV1, InclusiveRange, LeverageFactor,
        MintDistributionV1, MintRate, PartsPerHundredThousands, PoolParameters, RewardPeriodLength,
        TimeParameters, TimeoutParameters, TransactionFeeDistribution, UpdateKeysIndex,
        UpdateKeysThreshold, ValidatorScoreParameters,
    },
};

// ── Public types ──────────────────────────────────────────────────────────────

/// A funded account produced at genesis.
///
/// Wrapped in an async-aware mutex so tests can hold exclusive access while
/// submitting a transaction without blocking the async executor.
pub type LockedAccount = tokio::sync::Mutex<concordium_rust_sdk::genesis::types::GenesisAccount>;

/// A validator credential set produced at genesis.
///
/// Wrapped in `Mutex<_>` for the same reason as [`LockedAccount`].
pub type LockedValidator = std::sync::Mutex<BakerCredentials>;

/// Everything needed to start and operate the private chain.
pub struct GenesisFixture {
    /// Genesis block, can be serialized and stored as `genesis.dat` and used by a node.
    pub genesis_data: genesis::GenesisData,
    /// Validator credentials.
    pub validators: Vec<LockedValidator>,
    /// Funded non-validator accounts with signing keys.
    pub accounts: Vec<LockedAccount>,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl GenesisFixture {
    /// Generate a fresh P10 single-validator genesis block timestamped to now.
    ///
    /// Creates:
    /// - 1 validator account (staking 3 500 000 CCD)
    /// - 1 funded regular account (100 000 CCD)
    /// - 1 identity provider and 1 anonymity revoker
    /// - Governance keys with 2-of-3 thresholds
    ///
    /// Everything is computed in memory; no files are read or written.
    pub fn single_validator() -> Result<Self> {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;

        let output = genesis::genesis_builder_p10()
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
            })
            // Additional funded account for sending transactions
            .generate_accounts(FreshAccountConfig {
                count: 1,
                balance: Amount::from_micro_ccd(100_000_000_000u64),
                stake: None,
                num_keys: 1,
                threshold: SignatureThreshold::ONE,
                identity_provider: IpIdentity::from(0),
                restake_earnings: false,
                foundation: false,
            })
            .generate_governance_keys(make_governance_keys())
            .with_protocol(make_protocol_params(now_ms))
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
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_access(keys: &[u16], threshold: u16) -> Level2AccessConfig {
    Level2AccessConfig {
        authorized_keys: keys.iter().map(|&i| UpdateKeysIndex { index: i }).collect(),
        threshold: UpdateKeysThreshold::try_from(threshold).unwrap(),
    }
}

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

fn make_protocol_params(genesis_time_ms: u64) -> ProtocolParamsCPV3 {
    ProtocolParamsCPV3 {
        core: CoreGenesisParametersV1 {
            genesis_time: Timestamp {
                millis: genesis_time_ms,
            },
            epoch_duration: Duration::from_millis(3_600_000),
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
                min_finalizers: 1,
                max_finalizers: 1,
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
