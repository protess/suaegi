//! VERBATIM port of Orca's `src/shared/setup-runner-command.ts` (87L) and
//! `src/shared/setup-agent-sequencing.ts` (257L) @ v1.4.146-rc.0.
//!
//! These two modules build the SHELL COMMAND STRINGS that gate an agent's
//! startup command behind a repo setup script's completion — a posix
//! (`bash -lc ...`) gate and a Windows (`cmd.exe`/PowerShell) gate, each with
//! their own quoting/escaping rules. Quoting and escaping correctness is the
//! highest-value axis in this crate; see each module's doc comment for the
//! full K1-K14 trap catalog from
//! `docs/superpowers/plans/2026-07-27-setup-agent-sequencing.md`.
//!
//! `setup_runner_command` resolves a runner script path + platform hint into
//! a concrete shell invocation (bash vs cmd.exe, with the WSL-UNC-to-Linux
//! conversion). `setup_agent_sequencing` builds on that resolution to
//! produce a matched pair of setup/startup commands synchronized through a
//! nonce-tagged completion marker file.

pub mod setup_agent_sequencing;
pub mod setup_runner_command;

pub use setup_agent_sequencing::{
    create_sequenced_setup_agent_commands, create_setup_agent_sequence_nonce,
    get_setup_agent_sequence_shell_for_tests, resolve_setup_agent_sequence_launch_command,
    CreateSequencedSetupAgentCommandsArgs, SequencedSetupAgentCommands,
    SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV,
};
pub use setup_runner_command::{
    build_setup_runner_command, get_setup_runner_command_platform_for_path, is_wsl_unc_path,
    resolve_setup_runner_command, wsl_unc_to_linux_path, SetupRunnerCommandPlatform,
    SetupRunnerCommandResolution, SetupRunnerCommandShell,
};
