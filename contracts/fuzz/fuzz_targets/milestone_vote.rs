#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String as SorobanString, Vec};
use stellar_grants::{StellarGrantsContract, StellarGrantsContractClient};

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }

    let result = std::panic::catch_unwind(|| {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(StellarGrantsContract, ());
        let client = StellarGrantsContractClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let token = Address::generate(&env);
        let reviewer = Address::generate(&env);
        let mut reviewers: Vec<Address> = Vec::new(&env);
        reviewers.push_back(reviewer.clone());

        let num_milestones = (data[0] % 8) as u32 + 1;
        let grant_id = match client.try_grant_create(
            &owner,
            &SorobanString::from_str(&env, "Fuzz Grant"),
            &SorobanString::from_str(&env, "Fuzzing"),
            &token,
            &100i128,
            &10i128,
            &num_milestones,
            &reviewers,
        ) {
            Ok(Ok(id)) => id,
            _ => return,
        };

        // Milestone index derived from input bytes
        let milestone_idx = (data[1] as u32) % (num_milestones + 2);

        // Submit milestone
        let desc = SorobanString::from_str(&env, "desc");
        let proof = SorobanString::from_str(&env, "proof");
        let _ = client.try_milestone_submit(&grant_id, &milestone_idx, &owner, &desc, &proof);

        // Vote on milestone
        let approve = data[2].is_multiple_of(2);
        let voter = if data.len() > 3 && data[3].is_multiple_of(4) {
            Address::generate(&env)
        } else {
            reviewer.clone()
        };

        let _ = client.try_milestone_vote(&grant_id, &milestone_idx, &voter, &approve, &None);

        if let Ok(Ok(milestone)) = client.try_get_milestone(&grant_id, &milestone_idx) {
            let count = milestone
                .votes
                .iter()
                .filter(|(v, _)| v == &reviewer)
                .count();
            assert!(count <= 1, "Double vote invariant violated!");
        }
    });

    if result.is_err() {
        panic!("Fuzz target panicked on non-empty input");
    }
});
