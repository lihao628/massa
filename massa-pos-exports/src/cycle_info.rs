use bitvec::vec::BitVec;
use massa_hash::{HashXof, HashXofDeserializer, HashXofSerializer, HASH_XOF_SIZE_BYTES};
use massa_models::{
    address::{Address, AddressDeserializer, AddressSerializer},
    prehash::PreHashMap,
    serialization::{BitVecDeserializer, BitVecSerializer},
};
use massa_serialization::{
    Deserializer, OptionDeserializer, OptionSerializer, SerializeError, Serializer,
    U64VarIntDeserializer, U64VarIntSerializer,
};
use nom::{
    branch::alt,
    bytes::complete::tag,
    combinator::value,
    error::{context, ContextError, ParseError},
    multi::length_count,
    sequence::tuple,
    IResult, Parser,
};
use num::rational::Ratio;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::ops::Bound::Included;

/// State of a cycle for all threads
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleInfo {
    /// cycle number
    pub cycle: u64,
    /// whether the cycle is complete (all slots final)
    pub complete: bool,
    /// number of rolls each staking address has
    pub roll_counts: BTreeMap<Address, u64>,
    /// random seed bits of all slots in the cycle so far
    pub rng_seed: BitVec<u8>,
    /// Per-address production statistics
    pub production_stats: PreHashMap<Address, ProductionStats>,
    /// Snapshot of the final state hash
    /// Used for PoS selections
    pub final_state_hash_snapshot: Option<HashXof<HASH_XOF_SIZE_BYTES>>,
}

impl CycleInfo {
    /// Create a new `CycleInfo`
    pub fn new(
        cycle: u64,
        complete: bool,
        roll_counts: BTreeMap<Address, u64>,
        rng_seed: BitVec<u8>,
        production_stats: PreHashMap<Address, ProductionStats>,
    ) -> Self {
        // create the new cycle
        CycleInfo {
            cycle,
            complete,
            roll_counts,
            rng_seed,
            production_stats,
            final_state_hash_snapshot: None,
        }
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Serializer for `CycleInfo`
pub struct CycleInfoSerializer {
    pub u64_ser: U64VarIntSerializer,
    pub bitvec_ser: BitVecSerializer,
    pub production_stats_ser: ProductionStatsSerializer,
    pub address_ser: AddressSerializer,
    pub opt_hash_ser: OptionSerializer<HashXof<HASH_XOF_SIZE_BYTES>, HashXofSerializer>,
}

impl Default for CycleInfoSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl CycleInfoSerializer {
    /// Creates a new `CycleInfo` serializer
    pub fn new() -> Self {
        Self {
            u64_ser: U64VarIntSerializer::new(),
            bitvec_ser: BitVecSerializer::new(),
            production_stats_ser: ProductionStatsSerializer::new(),
            address_ser: AddressSerializer::new(),
            opt_hash_ser: OptionSerializer::new(HashXofSerializer::new()),
        }
    }
}

impl Serializer<CycleInfo> for CycleInfoSerializer {
    fn serialize(&self, value: &CycleInfo, buffer: &mut Vec<u8>) -> Result<(), SerializeError> {
        // cycle_info.cycle
        self.u64_ser.serialize(&value.cycle, buffer)?;

        // cycle_info.complete
        buffer.push(u8::from(value.complete));

        // cycle_info.roll_counts
        self.u64_ser
            .serialize(&(value.roll_counts.len() as u64), buffer)?;
        for (addr, count) in &value.roll_counts {
            self.address_ser.serialize(addr, buffer)?;
            self.u64_ser.serialize(count, buffer)?;
        }

        // cycle_info.rng_seed
        self.bitvec_ser.serialize(&value.rng_seed, buffer)?;

        // cycle_info.production_stats
        self.production_stats_ser
            .serialize(&value.production_stats, buffer)?;

        // cycle_info.final_state_hash_snapshot
        self.opt_hash_ser
            .serialize(&value.final_state_hash_snapshot, buffer)?;

        Ok(())
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Deserializer for `CycleInfo`
pub struct CycleInfoDeserializer {
    pub u64_deser: U64VarIntDeserializer,
    pub rolls_deser: RollsDeserializer,
    pub bitvec_deser: BitVecDeserializer,
    pub production_stats_deser: ProductionStatsDeserializer,
    pub opt_hash_deser: OptionDeserializer<HashXof<HASH_XOF_SIZE_BYTES>, HashXofDeserializer>,
}

impl CycleInfoDeserializer {
    /// Creates a new `CycleInfo` deserializer
    pub fn new(max_rolls_length: u64, max_production_stats_length: u64) -> CycleInfoDeserializer {
        CycleInfoDeserializer {
            u64_deser: U64VarIntDeserializer::new(Included(u64::MIN), Included(u64::MAX)),
            rolls_deser: RollsDeserializer::new(max_rolls_length),
            bitvec_deser: BitVecDeserializer::new(),
            production_stats_deser: ProductionStatsDeserializer::new(max_production_stats_length),
            opt_hash_deser: OptionDeserializer::new(HashXofDeserializer::new()),
        }
    }
}

impl Deserializer<CycleInfo> for CycleInfoDeserializer {
    fn deserialize<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
        &self,
        buffer: &'a [u8],
    ) -> IResult<&'a [u8], CycleInfo, E> {
        context(
            "cycle_history",
            tuple((
                context("cycle", |input| self.u64_deser.deserialize(input)),
                context(
                    "complete",
                    alt((value(true, tag(&[1])), value(false, tag(&[0])))),
                ),
                context("roll_counts", |input| self.rolls_deser.deserialize(input)),
                context("rng_seed", |input| self.bitvec_deser.deserialize(input)),
                context("production_stats", |input| {
                    self.production_stats_deser.deserialize(input)
                }),
                context("final_state_hash_snapshot", |input| {
                    self.opt_hash_deser.deserialize(input)
                }),
            )),
        )
        .map(
            #[allow(clippy::type_complexity)]
            |(cycle, complete, roll_counts, rng_seed, production_stats, opt_hash): (
                u64,                                  // cycle
                bool,                                 // complete
                Vec<(Address, u64)>,                  // roll_counts
                BitVec<u8>,                           // rng_seed
                PreHashMap<Address, ProductionStats>, // production_stats (address, n_success, n_fail)
                Option<HashXof<HASH_XOF_SIZE_BYTES>>, // final_state_hash_snapshot
            )| {
                let mut cycle = CycleInfo::new(
                    cycle,
                    complete,
                    roll_counts.into_iter().collect(),
                    rng_seed,
                    production_stats,
                );
                cycle.final_state_hash_snapshot = opt_hash;
                cycle
            },
        )
        .parse(buffer)
    }
}

/// Block production statistics
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProductionStats {
    /// Number of successfully created blocks
    pub block_success_count: u64,
    /// Number of blocks missed
    pub block_failure_count: u64,
}

impl ProductionStats {
    /// Check if the production stats are above the required percentage
    pub fn is_satisfying(&self, max_miss_ratio: &Ratio<u64>) -> bool {
        let opportunities_count = self.block_success_count + self.block_failure_count;
        if opportunities_count == 0 {
            return true;
        }
        &Ratio::new(self.block_failure_count, opportunities_count) <= max_miss_ratio
    }

    /// Increment a production stat structure with another
    pub fn extend(&mut self, stats: &ProductionStats) {
        self.block_success_count = self
            .block_success_count
            .saturating_add(stats.block_success_count);
        self.block_failure_count = self
            .block_failure_count
            .saturating_add(stats.block_failure_count);
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Serializer for `ProductionStats`
pub struct ProductionStatsSerializer {
    pub u64_ser: U64VarIntSerializer,
    address_ser: AddressSerializer,
}

impl Default for ProductionStatsSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProductionStatsSerializer {
    /// Creates a new `ProductionStats` serializer
    pub fn new() -> Self {
        Self {
            u64_ser: U64VarIntSerializer::new(),
            address_ser: AddressSerializer::new(),
        }
    }
}

impl Serializer<PreHashMap<Address, ProductionStats>> for ProductionStatsSerializer {
    fn serialize(
        &self,
        value: &PreHashMap<Address, ProductionStats>,
        buffer: &mut Vec<u8>,
    ) -> Result<(), SerializeError> {
        self.u64_ser.serialize(&(value.len() as u64), buffer)?;
        for (
            addr,
            ProductionStats {
                block_success_count,
                block_failure_count,
            },
        ) in value.iter()
        {
            self.address_ser.serialize(addr, buffer)?;
            self.u64_ser.serialize(block_success_count, buffer)?;
            self.u64_ser.serialize(block_failure_count, buffer)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Deserializer for `ProductionStats`
pub struct ProductionStatsDeserializer {
    length_deserializer: U64VarIntDeserializer,
    pub address_deserializer: AddressDeserializer,
    pub u64_deserializer: U64VarIntDeserializer,
}

impl ProductionStatsDeserializer {
    /// Creates a new `ProductionStats` deserializer
    pub fn new(max_production_stats_length: u64) -> ProductionStatsDeserializer {
        ProductionStatsDeserializer {
            length_deserializer: U64VarIntDeserializer::new(
                Included(u64::MIN),
                Included(max_production_stats_length),
            ),
            address_deserializer: AddressDeserializer::new(),
            u64_deserializer: U64VarIntDeserializer::new(Included(u64::MIN), Included(u64::MAX)),
        }
    }
}

impl Deserializer<PreHashMap<Address, ProductionStats>> for ProductionStatsDeserializer {
    fn deserialize<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
        &self,
        buffer: &'a [u8],
    ) -> IResult<&'a [u8], PreHashMap<Address, ProductionStats>, E> {
        context(
            "Failed ProductionStats deserialization",
            length_count(
                context("Failed length deserialization", |input| {
                    self.length_deserializer.deserialize(input)
                }),
                tuple((
                    context("Failed address deserialization", |input| {
                        self.address_deserializer.deserialize(input)
                    }),
                    context("Failed block_success_count deserialization", |input| {
                        self.u64_deserializer.deserialize(input)
                    }),
                    context("Failed block_failure_count deserialization", |input| {
                        self.u64_deserializer.deserialize(input)
                    }),
                )),
            ),
        )
        .map(|elements| {
            elements
                .into_iter()
                .map(|(addr, block_success_count, block_failure_count)| {
                    (
                        addr,
                        ProductionStats {
                            block_success_count,
                            block_failure_count,
                        },
                    )
                })
                .collect()
        })
        .parse(buffer)
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Deserializer for rolls
pub struct RollsDeserializer {
    length_deserializer: U64VarIntDeserializer,
    pub address_deserializer: AddressDeserializer,
    pub u64_deserializer: U64VarIntDeserializer,
}

impl RollsDeserializer {
    /// Creates a new rolls deserializer
    pub fn new(max_rolls_length: u64) -> RollsDeserializer {
        RollsDeserializer {
            length_deserializer: U64VarIntDeserializer::new(
                Included(u64::MIN),
                Included(max_rolls_length),
            ),
            address_deserializer: AddressDeserializer::new(),
            u64_deserializer: U64VarIntDeserializer::new(Included(u64::MIN), Included(u64::MAX)),
        }
    }
}

impl Deserializer<Vec<(Address, u64)>> for RollsDeserializer {
    fn deserialize<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
        &self,
        buffer: &'a [u8],
    ) -> IResult<&'a [u8], Vec<(Address, u64)>, E> {
        context(
            "Failed rolls deserialization",
            length_count(
                context("Failed length deserialization", |input| {
                    self.length_deserializer.deserialize(input)
                }),
                tuple((
                    context("Failed address deserialization", |input| {
                        self.address_deserializer.deserialize(input)
                    }),
                    context("Failed number deserialization", |input| {
                        self.u64_deserializer.deserialize(input)
                    }),
                )),
            ),
        )
        .parse(buffer)
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Serializer for cycle history
pub struct CycleHistorySerializer {
    pub u64_serializer: U64VarIntSerializer,
    pub cycle_info_serializer: CycleInfoSerializer,
}

impl CycleHistorySerializer {
    /// Creates a new `CycleHistory` serializer
    pub fn new() -> Self {
        Self {
            u64_serializer: U64VarIntSerializer::new(),
            cycle_info_serializer: CycleInfoSerializer::new(),
        }
    }
}

impl Default for CycleHistorySerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl Serializer<VecDeque<CycleInfo>> for CycleHistorySerializer {
    fn serialize(
        &self,
        value: &VecDeque<CycleInfo>,
        buffer: &mut Vec<u8>,
    ) -> Result<(), SerializeError> {
        self.u64_serializer
            .serialize(&(value.len() as u64), buffer)?;
        for cycle_info in value.iter() {
            self.cycle_info_serializer.serialize(cycle_info, buffer)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
#[allow(missing_docs)]
/// Deserializer for cycle history, useful when restarting from a snapshot
pub struct CycleHistoryDeserializer {
    pub u64_deserializer: U64VarIntDeserializer,
    pub cycle_info_deserializer: CycleInfoDeserializer,
}

impl CycleHistoryDeserializer {
    /// Creates a new `CycleHistory` deserializer
    pub fn new(
        max_cycle_history_length: u64,
        max_rolls_length: u64,
        max_production_stats_length: u64,
    ) -> Self {
        Self {
            u64_deserializer: U64VarIntDeserializer::new(
                Included(u64::MIN),
                Included(max_cycle_history_length),
            ),
            cycle_info_deserializer: CycleInfoDeserializer::new(
                max_rolls_length,
                max_production_stats_length,
            ),
        }
    }
}

impl Deserializer<Vec<CycleInfo>> for CycleHistoryDeserializer {
    fn deserialize<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
        &self,
        buffer: &'a [u8],
    ) -> IResult<&'a [u8], Vec<CycleInfo>, E> {
        context(
            "Failed cycle_history deserialization",
            length_count(
                context("Failed length deserialization", |input| {
                    self.u64_deserializer.deserialize(input)
                }),
                context("Failed cycle_info deserialization", |input| {
                    self.cycle_info_deserializer.deserialize(input)
                }),
            ),
        )
        .parse(buffer)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use massa_models::config::{MAX_PRODUCTION_STATS_LENGTH, MAX_ROLLS_COUNT_LENGTH};
    use massa_serialization::DeserializeError;
    use std::str::FromStr;

    fn create_cycle_info() -> (CycleInfo, Address, Address) {
        let addr1 =
            Address::from_str("AU1jUbxeXW49QRT6Le5aPuNdcGWQV2kpnDyQkKoka4MmEUW3m8Xm").unwrap();
        let addr2 =
            Address::from_str("AU12nfJdBNotWffSEDDCS9mMXAxDbHbAVM9GW7pvVJoLxdCeeroX8").unwrap();
        let mut prod_stats = PreHashMap::default();
        prod_stats.insert(addr1, ProductionStats::default());
        prod_stats.insert(
            addr2,
            ProductionStats {
                block_success_count: 65539,
                block_failure_count: 2,
            },
        );

        let mut cycle_info1 = CycleInfo::new(
            0,
            false,
            BTreeMap::from([(addr1, 1), (addr2, 100)]),
            BitVec::new(),
            prod_stats,
        );
        cycle_info1.final_state_hash_snapshot = Some(HashXof::from_bytes(&[2u8; 512]));

        return (cycle_info1, addr1, addr2);
    }

    #[test]
    fn test_cycle_info_ser_der() {
        let (cycle_info1, _, _) = create_cycle_info();

        let mut buf = Vec::new();
        let serializer = CycleInfoSerializer::new();
        let deserializer =
            CycleInfoDeserializer::new(MAX_ROLLS_COUNT_LENGTH, MAX_PRODUCTION_STATS_LENGTH);

        serializer.serialize(&cycle_info1, &mut buf).unwrap();
        let (rem, cycle_der) = deserializer.deserialize::<DeserializeError>(&buf).unwrap();
        assert!(rem.is_empty());
        assert_eq!(cycle_der, cycle_info1);

        // With limits
        let deserializer2 = CycleInfoDeserializer::new(1, MAX_PRODUCTION_STATS_LENGTH);
        let deserializer3 = CycleInfoDeserializer::new(MAX_ROLLS_COUNT_LENGTH, 1);

        buf.clear();
        serializer.serialize(&cycle_info1, &mut buf).unwrap();
        let res2 = deserializer2.deserialize::<DeserializeError>(&buf);
        assert!(res2.is_err());
        buf.clear();
        serializer.serialize(&cycle_info1, &mut buf).unwrap();
        let res3 = deserializer3.deserialize::<DeserializeError>(&buf);
        assert!(res3.is_err());
    }

    #[test]
    fn test_cycle_history_ser_der() {
        let (mut cycle_info1, _addr1, _addr2) = create_cycle_info();
        let mut cycle_info2 = cycle_info1.clone();
        cycle_info2.cycle += 1;
        cycle_info1.complete = true;

        let serializer = CycleHistorySerializer::new();
        let deserializer =
            CycleHistoryDeserializer::new(2, MAX_ROLLS_COUNT_LENGTH, MAX_PRODUCTION_STATS_LENGTH);
        let deserializer2 =
            CycleHistoryDeserializer::new(1, MAX_ROLLS_COUNT_LENGTH, MAX_PRODUCTION_STATS_LENGTH);
        let cycle_history = VecDeque::from([cycle_info1, cycle_info2]);

        let mut buf = Vec::new();
        serializer.serialize(&cycle_history, &mut buf).unwrap();
        let (rem, cycle_history_der) = deserializer.deserialize::<DeserializeError>(&buf).unwrap();
        assert!(rem.is_empty());
        assert_eq!(
            cycle_history_der,
            cycle_history
                .clone()
                .into_iter()
                .collect::<Vec<CycleInfo>>()
        );

        buf.clear();
        serializer.serialize(&cycle_history, &mut buf).unwrap();
        let res2 = deserializer2.deserialize::<DeserializeError>(&buf);
        assert!(res2.is_err());
    }
}
