use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

use cairo_lang_compiler::db::RootDatabase;
use cairo_lang_compiler::diagnostics::DiagnosticsReporter;
use cairo_lang_filesystem::db::FilesGroupEx;
use cairo_lang_filesystem::flag::Flag;
use cairo_lang_filesystem::ids::FlagId;
use cairo_lang_semantic::test_utils::setup_test_module;
use cairo_lang_sierra::extensions::gas::CostTokenType;
use cairo_lang_sierra::ids::FunctionId;
use cairo_lang_sierra::program::{Function, Program};
use cairo_lang_sierra_generator::db::SierraGenGroup;
use cairo_lang_sierra_generator::replace_ids::replace_sierra_ids_in_program;
use cairo_lang_sierra_to_casm::metadata::{calc_metadata, MetadataComputationConfig};
use cairo_lang_test_utils::parse_test_file::{TestFileRunner, TestRunnerResult};
use cairo_lang_test_utils::test_lock;
use cairo_lang_utils::ordered_hash_map::OrderedHashMap;
use cairo_lang_utils::unordered_hash_map::UnorderedHashMap;
use cairo_lang_utils::Upcast;
use itertools::Itertools;
use once_cell::sync::Lazy;

/// Salsa databases configured to find the corelib, when reused by different tests should be able to
/// use the cached queries that rely on the corelib's code, which vastly reduces the tests runtime.
static SHARED_DB_WITH_GAS: Lazy<Mutex<RootDatabase>> =
    Lazy::new(|| Mutex::new(RootDatabase::builder().detect_corelib().build().unwrap()));
static SHARED_DB_NO_GAS: Lazy<Mutex<RootDatabase>> = Lazy::new(|| {
    let mut db = RootDatabase::builder().detect_corelib().build().unwrap();
    let add_withdraw_gas_flag_id = FlagId::new(db.upcast(), "add_withdraw_gas");
    db.set_flag(add_withdraw_gas_flag_id, Some(Arc::new(Flag::AddWithdrawGas(false))));
    Mutex::new(db)
});

cairo_lang_test_utils::test_file_test_with_runner!(
    general_e2e,
    "e2e_test_data",
    {
        cmp: "cmp",
    },
    SmallE2ETestRunner
);

cairo_lang_test_utils::test_file_test_with_runner!(
    libfunc_e2e,
    "e2e_test_data/libfuncs",
    {
        array: "array",
        bitwise: "bitwise",
        bool: "bool",
        box_: "box",
        builtin_costs: "builtin_costs",
        casts: "casts",
        ec: "ec",
        enum_: "enum",
        enum_snapshot: "enum_snapshot",
        felt252_dict: "felt252_dict",
        felt252: "felt252",
        i128: "i128",
        i16: "i16",
        i32: "i32",
        i64: "i64",
        i8: "i8",
        nullable: "nullable",
        poseidon: "poseidon",
        snapshot: "snapshot",
        u128: "u128",
        u16: "u16",
        bytes31: "bytes31",
        u256: "u256",
        u32: "u32",
        u512: "u512",
        u64: "u64",
        u8: "u8",
    },
    SmallE2ETestRunner
);

cairo_lang_test_utils::test_file_test_with_runner!(
    libfunc_e2e_skip_add_gas,
    "e2e_test_data/libfuncs",
    {
        gas: "gas",
    },
    SmallE2ETestRunnerSkipAddGas
);

cairo_lang_test_utils::test_file_test_with_runner!(
    starknet_libfunc_e2e,
    "e2e_test_data/libfuncs/starknet",
    {
        class_hash: "class_hash",
        contract_address: "contract_address",
        secp256k1: "secp256k1",
        secp256r1: "secp256r1",
        storage_address: "storage_address",
        syscalls: "syscalls",
    },
    SmallE2ETestRunner
);

cairo_lang_test_utils::test_file_test_with_runner!(
    cost_computation_e2e,
    "e2e_test_data",
    {
        cost_computation: "cost_computation",
    },
    SmallE2ETestRunnerCostComputation
);

#[derive(Default)]
struct SmallE2ETestRunner;
impl TestFileRunner for SmallE2ETestRunner {
    fn run(
        &mut self,
        inputs: &OrderedHashMap<String, String>,
        _args: &OrderedHashMap<String, String>,
    ) -> TestRunnerResult {
        run_e2e_test(inputs, E2eTestParams::default())
    }
}

#[derive(Default)]
struct SmallE2ETestRunnerSkipAddGas;
impl TestFileRunner for SmallE2ETestRunnerSkipAddGas {
    fn run(
        &mut self,
        inputs: &OrderedHashMap<String, String>,
        _args: &OrderedHashMap<String, String>,
    ) -> TestRunnerResult {
        run_e2e_test(inputs, E2eTestParams { add_withdraw_gas: false, ..E2eTestParams::default() })
    }
}

#[derive(Default)]
struct SmallE2ETestRunnerCostComputation;
impl TestFileRunner for SmallE2ETestRunnerCostComputation {
    fn run(
        &mut self,
        inputs: &OrderedHashMap<String, String>,
        _args: &OrderedHashMap<String, String>,
    ) -> TestRunnerResult {
        run_e2e_test(inputs, E2eTestParams { add_withdraw_gas: false, cost_computation: true })
    }
}

/// Represents the parameters of `run_e2e_test`.
struct E2eTestParams {
    /// Argument for `run_e2e_test` that controls whether to set the `add_withdraw_gas` flag
    /// that automatically adds `withdraw_gas` calls.
    add_withdraw_gas: bool,

    /// Argument for `run_e2e_test` that controls whether to add cost computation information to
    /// the test outputs.
    cost_computation: bool,
}

/// Implements default for `E2eTestParams`.
impl Default for E2eTestParams {
    fn default() -> Self {
        Self { add_withdraw_gas: true, cost_computation: false }
    }
}

/// Runs the e2e test.
fn run_e2e_test(
    inputs: &OrderedHashMap<String, String>,
    params: E2eTestParams,
) -> TestRunnerResult {
    let mut locked_db =
        test_lock(if params.add_withdraw_gas { &SHARED_DB_WITH_GAS } else { &SHARED_DB_NO_GAS });
    // Parse code and create semantic model.
    let test_module = setup_test_module(locked_db.deref_mut(), inputs["cairo"].as_str()).unwrap();
    let db = locked_db.snapshot();
    DiagnosticsReporter::stderr().with_extra_crates(&[test_module.crate_id]).ensure(&db).unwrap();

    // Compile to Sierra.
    let sierra_program = db.get_sierra_program(vec![test_module.crate_id]).unwrap();
    let sierra_program = replace_sierra_ids_in_program(&db, &sierra_program);
    let sierra_program_str = sierra_program.to_string();

    // Handle the `enforced_costs` argument.
    let enforced_costs: OrderedHashMap<FunctionId, OrderedHashMap<CostTokenType, i32>> =
        if let Some(enforced_costs_str) = inputs.get("enforced_costs") {
            parse_enforced_costs(&sierra_program, enforced_costs_str)
        } else {
            Default::default()
        };

    // Compute the metadata.
    let metadata_config = MetadataComputationConfig { function_set_costs: enforced_costs };
    let metadata = calc_metadata(&sierra_program, metadata_config.clone(), false).unwrap();

    // Compile to casm.
    let casm = cairo_lang_sierra_to_casm::compiler::compile(&sierra_program, &metadata, true)
        .unwrap()
        .to_string();

    let mut res: OrderedHashMap<String, String> =
        OrderedHashMap::from([("casm".into(), casm), ("sierra_code".into(), sierra_program_str)]);
    if params.cost_computation {
        let metadata_no_solver = calc_metadata(&sierra_program, metadata_config, true).unwrap();
        res.insert("gas_solution".into(), format!("{}", metadata.gas_info));
        res.insert("gas_solution_no_solver".into(), format!("{}", metadata_no_solver.gas_info));

        // Compile again, this time with the no-solver metadata.
        cairo_lang_sierra_to_casm::compiler::compile(&sierra_program, &metadata_no_solver, true)
            .unwrap();
    } else {
        let function_costs_str = metadata
            .gas_info
            .function_costs
            .iter()
            .map(|(func_id, cost)| format!("{func_id}: {cost:?}"))
            .join("\n");
        res.insert("function_costs".into(), function_costs_str.to_string());
    }

    TestRunnerResult::success(res)
}

/// Parses the `enforced_costs` test argument. It should consist of lines of the form
///   <function_name> <cost>
/// Where `function_name` is the fully-qualified name of the function, and `cost` is the cost to
/// enforce for that function.
fn parse_enforced_costs(
    sierra_program: &Program,
    enforced_costs_str: &str,
) -> OrderedHashMap<FunctionId, OrderedHashMap<CostTokenType, i32>> {
    // Create a map from function name to function id.
    let function_name_to_id: UnorderedHashMap<&str, _> = sierra_program
        .funcs
        .iter()
        .map(|Function { id, .. }| (id.debug_name.as_ref().unwrap().as_str(), id))
        .collect();

    enforced_costs_str
        .split('\n')
        .map(|line| {
            // line is the name of the function and the enforced cost, separated by a space.
            let [name, cost_str] = line.split(' ').collect_vec()[..] else {
                panic!(
                    "Invalid enforced cost line. Expected a line of the form '<function name> \
                     <cost>'."
                );
            };

            // Get the FunctionId from the name by searching program.funcs.
            let function_id = *function_name_to_id
                .get(name)
                .unwrap_or_else(|| panic!("Function {name} was not found."));
            let cost = cost_str
                .parse::<i32>()
                .unwrap_or_else(|_| panic!("Expected a number as the enforced cost."));
            (function_id.clone(), [(CostTokenType::Const, cost)].into_iter().collect())
        })
        .collect::<OrderedHashMap<_, _>>()
}
