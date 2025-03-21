use anchor_lang::prelude::*;
use anchor_spl::token::{self, Transfer, Token, TokenAccount, Burn, Mint};

declare_id!("GSJ1Uj1xh4LMEWAssmpni4i4HaXj7GLHM54BBcC9VTRK");

#[program]
pub mod zkarb {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, fee_multiplier: u64) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.admin = ctx.accounts.admin.key();
        pool.total_staked = 0;
        pool.total_liquidity = 0;
        pool.accumulated_fee_tokens = 0;
        pool.dynamic_fee_multiplier = fee_multiplier;
        Ok(())
    }

    pub fn stake_tokens(ctx: Context<StakeTokens>, amount: u64) -> Result<()> {
        // Transfer tokens from user's token account to the staking vault.
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.staking_vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Update stake account.
        let stake_account = &mut ctx.accounts.stake_account;
        stake_account.owner = ctx.accounts.user.key();
        stake_account.amount = stake_account
            .amount
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        let current_time = Clock::get()?.unix_timestamp;
        stake_account.staked_at = current_time;
        stake_account.lockup_until = current_time + 300; // 5-minute lockup

        // Update pool state.
        ctx.accounts.pool.total_staked = ctx.accounts.pool.total_staked
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        emit!(StakeDeposited {
            user: ctx.accounts.user.key(),
            amount,
        });
        Ok(())
    }

    pub fn withdraw_stake(ctx: Context<WithdrawStake>, amount: u64) -> Result<()> {
        // Manual check: ensure that the stake account owner matches the withdrawing signer.
        require!(
            ctx.accounts.stake_account.owner == ctx.accounts.user.key(),
            CustomError::Unauthorized
        );

        let stake_account = &mut ctx.accounts.stake_account;
        let current_time = Clock::get()?.unix_timestamp;
        require!(
            current_time >= stake_account.lockup_until,
            CustomError::LockupPeriodNotExpired
        );
        require!(
            stake_account.amount >= amount,
            CustomError::InsufficientStakedBalance
        );

        stake_account.amount = stake_account
            .amount
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        ctx.accounts.pool.total_staked = ctx.accounts.pool.total_staked
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;

        // Transfer tokens from staking vault back to user's token account.
        let pool_key = ctx.accounts.pool.key();
        let seeds = &[b"staking_vault", pool_key.as_ref(), &[ctx.accounts.pool.staking_vault_bump]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.staking_vault.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer,
            ),
            amount,
        )?;

        // Emit bonus reward eligibility if staked more than 7 days.
        if current_time - stake_account.staked_at >= 604800 {
            emit!(BonusRewardEligible {
                user: ctx.accounts.user.key(),
                bonus: amount / 20, // Example: 5% bonus.
            });
        }
        emit!(StakeWithdrawn {
            user: ctx.accounts.user.key(),
            amount,
        });
        Ok(())
    }

    pub fn add_liquidity(ctx: Context<AddLiquidity>, amount: u64) -> Result<()> {
        // Manual check: ensure the liquidity provider account's owner matches the signer.
        require!(
            ctx.accounts.liquidity_provider_account.owner == ctx.accounts.liquidity_provider.key(),
            CustomError::Unauthorized
        );

        let cpi_accounts = Transfer {
            from: ctx.accounts.lp_token_account.to_account_info(),
            to: ctx.accounts.liquidity_vault.to_account_info(),
            authority: ctx.accounts.liquidity_provider.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        let lp_account = &mut ctx.accounts.liquidity_provider_account;
        lp_account.amount = lp_account
            .amount
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        ctx.accounts.pool.total_liquidity = ctx.accounts.pool.total_liquidity
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        emit!(LiquidityDeposited {
            liquidity_provider: ctx.accounts.liquidity_provider.key(),
            amount,
        });
        Ok(())
    }

    pub fn remove_liquidity(ctx: Context<RemoveLiquidity>, amount: u64) -> Result<()> {
        // Manual check: ensure the liquidity provider account's owner matches the signer.
        require!(
            ctx.accounts.liquidity_provider_account.owner == ctx.accounts.liquidity_provider.key(),
            CustomError::Unauthorized
        );

        let lp_account = &mut ctx.accounts.liquidity_provider_account;
        require!(
            lp_account.amount >= amount,
            CustomError::InsufficientLiquidity
        );
        lp_account.amount = lp_account
            .amount
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        ctx.accounts.pool.total_liquidity = ctx.accounts.pool.total_liquidity
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;

        let pool_key = ctx.accounts.pool.key();
        let seeds = &[b"liquidity_vault", pool_key.as_ref(), &[ctx.accounts.pool.liquidity_vault_bump]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.liquidity_vault.to_account_info(),
            to: ctx.accounts.lp_token_account.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer,
            ),
            amount,
        )?;
        emit!(LiquidityRemoved {
            liquidity_provider: ctx.accounts.liquidity_provider.key(),
            amount,
        });
        Ok(())
    }

    pub fn approve_liquidity_provider(ctx: Context<ApproveLiquidityProvider>) -> Result<()> {
        // Manual check: ensure the liquidity provider account's owner matches the signer.
        require!(
            ctx.accounts.liquidity_provider_account.owner == ctx.accounts.liquidity_provider.key(),
            CustomError::Unauthorized
        );
        let lp_account = &mut ctx.accounts.liquidity_provider_account;
        lp_account.approved = true;
        emit!(LiquidityProviderApproved {
            liquidity_provider: ctx.accounts.liquidity_provider.key(),
        });
        Ok(())
    }

    pub fn execute_arbitrage(
        ctx: Context<ExecuteArbitrage>,
        amount: u64,
        min_profit: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        let _delay = get_random_delay();

        require!(
            verify_zk_snark(&zk_proof),
            CustomError::InvalidProof
        );

        let actual_profit = get_arbitrage_profit(amount);
        require!(
            actual_profit >= min_profit,
            CustomError::SlippageTooHigh
        );

        let fee_in_lamports = actual_profit
            .checked_mul(ctx.accounts.pool.dynamic_fee_multiplier)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(1000)
            .ok_or(ErrorCode::MathOverflow)?;
        **ctx.accounts.trader.to_account_info().try_borrow_mut_lamports()? = ctx
            .accounts
            .trader
            .lamports()
            .checked_sub(fee_in_lamports)
            .ok_or(ErrorCode::MathOverflow)?;
        **ctx.accounts.fee_vault_sol.to_account_info().try_borrow_mut_lamports()? = ctx
            .accounts
            .fee_vault_sol
            .lamports()
            .checked_add(fee_in_lamports)
            .ok_or(ErrorCode::MathOverflow)?;

        let fee_tokens = actual_profit
            .checked_mul(ctx.accounts.pool.dynamic_fee_multiplier)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(1000)
            .ok_or(ErrorCode::MathOverflow)?;
        ctx.accounts.pool.accumulated_fee_tokens = ctx.accounts.pool.accumulated_fee_tokens
            .checked_add(fee_tokens)
            .ok_or(ErrorCode::MathOverflow)?;

        emit!(ArbitrageExecuted {
            trader: ctx.accounts.trader.key(),
            amount,
            profit: actual_profit.checked_sub(fee_tokens).unwrap_or(0),
        });

        distribute_rewards(&mut ctx.accounts.pool, actual_profit.checked_sub(fee_tokens).unwrap_or(0))?;
        Ok(())
    }

    pub fn rebalance_liquidity(ctx: Context<RebalanceLiquidity>) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        let optimal_liquidity = get_optimal_liquidity();
        pool.total_liquidity = optimal_liquidity;
        emit!(LiquidityRebalanced {
            new_liquidity: optimal_liquidity,
        });
        Ok(())
    }

    pub fn update_fee_multiplier(ctx: Context<UpdateFeeMultiplier>, new_multiplier: u64) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        require!(
            ctx.accounts.admin.key() == pool.admin,
            CustomError::Unauthorized
        );
        pool.dynamic_fee_multiplier = new_multiplier;
        emit!(FeeMultiplierUpdated { new_multiplier });
        Ok(())
    }

    pub fn burn_fee_tokens(ctx: Context<BurnFeeTokens>, amount: u64) -> Result<()> {
        let pool_key = ctx.accounts.pool.key();
        let seeds = &[b"fee_vault", pool_key.as_ref(), &[ctx.accounts.pool.fee_vault_bump]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Burn {
            mint: ctx.accounts.token_mint.to_account_info(),
            from: ctx.accounts.fee_vault.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        token::burn(
            CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer),
            amount,
        )?;
        emit!(FeeTokensBurned { amount });
        Ok(())
    }
}

/// Placeholder rewards distribution.
pub fn distribute_rewards(_pool: &mut Account<Pool>, _amount: u64) -> Result<()> {
    Ok(())
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = admin, space = 8 + Pool::LEN)]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = admin,
        token::mint = token_mint,
        token::authority = pool,
        seeds = [b"staking_vault", pool.key().as_ref()],
        bump
    )]
    pub staking_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        token::mint = token_mint,
        token::authority = pool,
        seeds = [b"liquidity_vault", pool.key().as_ref()],
        bump
    )]
    pub liquidity_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        token::mint = token_mint,
        token::authority = pool,
        seeds = [b"fee_vault", pool.key().as_ref()],
        bump
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = admin,
        seeds = [b"fee_vault_sol", pool.key().as_ref()],
        bump,
        space = 8
    )]
    pub fee_vault_sol: UncheckedAccount<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub token_mint: Box<Account<'info, Mint>>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct StakeTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    /// User's token account for $ZKARB.
    #[account(mut)]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    /// The staking vault PDA.
    #[account(mut, seeds = [b"staking_vault", pool.key().as_ref()], bump = pool.staking_vault_bump)]
    pub staking_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = user,
        space = 8 + StakeAccount::LEN,
        seeds = [b"stake", user.key().as_ref()],
        bump
    )]
    pub stake_account: Box<Account<'info, StakeAccount>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct WithdrawStake<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    /// User's token account to receive withdrawn tokens.
    #[account(mut)]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    /// The staking vault PDA.
    #[account(mut, seeds = [b"staking_vault", pool.key().as_ref()], bump = pool.staking_vault_bump)]
    pub staking_vault: Box<Account<'info, TokenAccount>>,

    // Removed "has_one" attribute; we'll check manually.
    #[account(mut)]
    pub stake_account: Box<Account<'info, StakeAccount>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    #[account(mut)]
    pub liquidity_provider: Signer<'info>,

    /// LP's token account for $ZKARB.
    #[account(mut)]
    pub lp_token_account: Box<Account<'info, TokenAccount>>,

    /// The liquidity vault PDA.
    #[account(mut, seeds = [b"liquidity_vault", pool.key().as_ref()], bump = pool.liquidity_vault_bump)]
    pub liquidity_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        init,
        payer = liquidity_provider,
        space = 8 + LiquidityProvider::LEN,
        seeds = [b"lp", liquidity_provider.key().as_ref()],
        bump
    )]
    pub liquidity_provider_account: Box<Account<'info, LiquidityProvider>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct RemoveLiquidity<'info> {
    #[account(mut)]
    pub liquidity_provider: Signer<'info>,

    /// LP's token account to receive tokens.
    #[account(mut)]
    pub lp_token_account: Box<Account<'info, TokenAccount>>,

    /// The liquidity vault PDA.
    #[account(mut, seeds = [b"liquidity_vault", pool.key().as_ref()], bump = pool.liquidity_vault_bump)]
    pub liquidity_vault: Box<Account<'info, TokenAccount>>,

    // Removed "has_one"; manual check in the instruction.
    #[account(mut)]
    pub liquidity_provider_account: Box<Account<'info, LiquidityProvider>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ApproveLiquidityProvider<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    // Removed "has_one"; manual check in the instruction.
    #[account(mut)]
    pub liquidity_provider_account: Box<Account<'info, LiquidityProvider>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    // The liquidity provider signer we want to check against.
    #[account(mut)]
    pub liquidity_provider: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecuteArbitrage<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    /// The SOL fee vault PDA.
    #[account(mut, seeds = [b"fee_vault_sol", pool.key().as_ref()], bump)]
    pub fee_vault_sol: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RebalanceLiquidity<'info> {
    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,
}

#[derive(Accounts)]
pub struct UpdateFeeMultiplier<'info> {
    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct BurnFeeTokens<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// The fee vault PDA from which tokens will be burned.
    #[account(mut, seeds = [b"fee_vault", pool.key().as_ref()], bump = pool.fee_vault_bump)]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub pool: Box<Account<'info, Pool>>,

    pub token_mint: Box<Account<'info, Mint>>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct Pool {
    pub admin: Pubkey,
    pub total_staked: u64,
    pub total_liquidity: u64,
    pub accumulated_fee_tokens: u64,
    pub dynamic_fee_multiplier: u64,
    pub staking_vault_bump: u8,
    pub liquidity_vault_bump: u8,
    pub fee_vault_bump: u8,
}

impl Pool {
    const LEN: usize = 32 + 8 + 8 + 8 + 8 + 1 + 1 + 1;
}

#[account]
pub struct StakeAccount {
    pub owner: Pubkey,
    pub amount: u64,
    pub staked_at: i64,
    pub lockup_until: i64,
}

impl StakeAccount {
    const LEN: usize = 32 + 8 + 8 + 8;
}

#[account]
pub struct LiquidityProvider {
    pub owner: Pubkey,
    pub amount: u64,
    pub approved: bool,
}

impl LiquidityProvider {
    const LEN: usize = 32 + 8 + 1;
}

#[error_code]
pub enum CustomError {
    #[msg("Invalid ZK proof provided.")]
    InvalidProof,
    #[msg("Slippage is too high; arbitrage not profitable.")]
    SlippageTooHigh,
    #[msg("Lockup period has not expired yet.")]
    LockupPeriodNotExpired,
    #[msg("Insufficient staked balance for withdrawal.")]
    InsufficientStakedBalance,
    #[msg("Insufficient liquidity available.")]
    InsufficientLiquidity,
    #[msg("Liquidity provider is not approved.")]
    NotApprovedLiquidityProvider,
    #[msg("Unauthorized access.")]
    Unauthorized,
}

#[error_code]
pub enum ErrorCode {
    #[msg("Math overflow occurred.")]
    MathOverflow,
}

#[event]
pub struct StakeDeposited {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct StakeWithdrawn {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct BonusRewardEligible {
    pub user: Pubkey,
    pub bonus: u64,
}

#[event]
pub struct LiquidityDeposited {
    pub liquidity_provider: Pubkey,
    pub amount: u64,
}

#[event]
pub struct LiquidityRemoved {
    pub liquidity_provider: Pubkey,
    pub amount: u64,
}

#[event]
pub struct LiquidityProviderApproved {
    pub liquidity_provider: Pubkey,
}

#[event]
pub struct ArbitrageExecuted {
    pub trader: Pubkey,
    pub amount: u64,
    pub profit: u64,
}

#[event]
pub struct LiquidityRebalanced {
    pub new_liquidity: u64,
}

#[event]
pub struct FeeMultiplierUpdated {
    pub new_multiplier: u64,
}

#[event]
pub struct FeeTokensBurned {
    pub amount: u64,
}

// ====================================================================
// Helper functions (placeholders – replace with production implementations)
// ====================================================================

fn get_random_delay() -> u64 {
    2 // Placeholder value.
}

fn verify_zk_snark(zk_proof: &Vec<u8>) -> bool {
    !zk_proof.is_empty()
}

fn get_arbitrage_profit(amount: u64) -> u64 {
    amount / 10 // Example: 10% profit.
}

fn get_optimal_liquidity() -> u64 {
    1_000_000 // Simulated optimal liquidity value.
}
