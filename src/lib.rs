use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, msg, pubkey::Pubkey,
};

entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let counter = instruction_data
        .get(..8)
        .map(|slice| u64::from_le_bytes(slice.try_into().unwrap()))
        .unwrap_or(0);
    msg!("Count: {}", counter);
    Ok(())
}

#[cfg(test)]
mod test {
    use solana_client::rpc_client::RpcClient;
    use solana_sdk::{
        bpf_loader_upgradeable::UpgradeableLoaderState,
        commitment_config::CommitmentConfig,
        instruction::Instruction,
        message::Message,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        transaction::Transaction,
    };
    use solana_transaction_status::{option_serializer::OptionSerializer, UiTransactionEncoding};
    use std::{str::FromStr, thread::sleep, time::Duration};
    use tracing::{info, warn};

    fn get_program_size(
        rpc_client: &RpcClient,
        program_id: &str,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let program_pubkey = Pubkey::from_str(program_id)?;
        let program_account = rpc_client.get_account(&program_pubkey)?;

        let (program_data_size, total_size) =
            if program_account.owner == solana_sdk::bpf_loader::id() {
                (program_account.data.len(), program_account.data.len())
            } else if program_account.owner == solana_sdk::bpf_loader_upgradeable::id() {
                // Deserialize the program account state
                let state: UpgradeableLoaderState = bincode::deserialize(&program_account.data)?;

                match state {
                    UpgradeableLoaderState::Program {
                        programdata_address,
                    } => {
                        let program_data_account = rpc_client.get_account(&programdata_address)?;
                        let program_data: UpgradeableLoaderState =
                            bincode::deserialize(&program_data_account.data)?;

                        match program_data {
                            UpgradeableLoaderState::ProgramData {
                                slot: _,
                                upgrade_authority_address: _,
                            } => {
                                let metadata_size =
                                    UpgradeableLoaderState::size_of_programdata_metadata();
                                let program_size = program_data_account.data.len() - metadata_size;
                                (program_size, program_data_account.data.len())
                            }
                            _ => return Err("Invalid program data account".into()),
                        }
                    }
                    _ => return Err("Not a program account".into()),
                }
            } else {
                return Err("Not a BPF program".into());
            };

        Ok((program_data_size, total_size))
    }

    #[test]
    fn test_multiple_transactions() {
        // Initialize tracing
        tracing_subscriber::fmt::init();

        let rpc_url = "http://127.0.0.1:8899".to_string();
        let rpc_client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

        let program_id = "H2GM7Vci4vVTWUfM4CQ5gWuUXF56wgXC4CkUY2Ea7mgC";

        // Get program size
        match get_program_size(&rpc_client, program_id) {
            Ok((program_size, total_size)) => {
                info!("Program data size: {} bytes", program_size);
                info!("Total account size: {} bytes", total_size);
            }
            Err(e) => warn!("Failed to get program size: {}", e),
        }

        let program_pubkey = program_id.parse().unwrap();
        let payer = Keypair::new();

        info!("Requesting airdrop for {}", payer.pubkey());
        let airdrop_signature = rpc_client
            .request_airdrop(&payer.pubkey(), 1_000_000_000)
            .unwrap();

        loop {
            if let Ok(_) = rpc_client.confirm_transaction(&airdrop_signature) {
                if let Ok(balance) = rpc_client.get_balance(&payer.pubkey()) {
                    if balance > 0 {
                        info!("Airdrop confirmed! Balance: {}", balance);
                        break;
                    }
                }
            }
            sleep(Duration::from_millis(100));
        }

        let mut signatures = Vec::with_capacity(100);

        // Get blockhash once before the loop
        let recent_blockhash = rpc_client.get_latest_blockhash().unwrap();

        // Send all transactions first
        for i in 0..100 {
            let instruction_data = (i as u64).to_le_bytes().to_vec();
            let instruction =
                Instruction::new_with_bytes(program_pubkey, &instruction_data, vec![]);

            let message = Message::new_with_blockhash(
                &[instruction],
                Some(&payer.pubkey()),
                &recent_blockhash,
            );
            let transaction = Transaction::new(&[&payer], message, recent_blockhash);

            match rpc_client.send_transaction(&transaction) {
                Ok(signature) => {
                    info!("Transaction {} sent: {}", i, signature);
                    signatures.push((i, signature));
                }
                Err(e) => warn!("Failed to send transaction {}: {}", i, e),
            }
        }

        info!("All transactions sent, now verifying...");

        for (i, signature) in signatures {
            let mut retries = 10;
            let mut tx_details = None;

            while retries > 0 {
                match rpc_client.get_transaction(&signature, UiTransactionEncoding::Base64) {
                    Ok(details) => {
                        tx_details = Some(details);
                        break;
                    }
                    Err(_) => {
                        sleep(Duration::from_millis(50));
                        retries -= 1;
                    }
                }
            }

            if let Some(details) = tx_details {
                if let Some(meta) = details.transaction.meta {
                    match meta.compute_units_consumed {
                        OptionSerializer::Some(cu) => {
                            info!(
                                "Transaction {} (counter: {}): Compute Units used: {}",
                                i + 1,
                                i,
                                cu
                            );
                        }
                        _ => warn!("Transaction {}: Compute units not available", i + 1),
                    }
                }
            }
        }
    }
}
