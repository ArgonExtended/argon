use anyhow::{bail, Result};
use clap::Parser;
use colored::Colorize;
use std::{fs::File, io::BufReader, path::PathBuf};

use crate::{
	argon_info,
	config::Config,
	core::Core,
	ext::PathExt,
	project::{self, Project},
};

/// Decompose a Roblox place or model file into the project's source tree
#[derive(Parser)]
pub struct Syncback {
	/// Input place or model file (.rbxl, .rbxlx, .rbxm, .rbxmx)
	#[arg()]
	input: PathBuf,

	/// Project path
	#[arg()]
	project: Option<PathBuf>,

	/// Remove instances that exist in the project but not in the input file
	#[arg(short, long)]
	prune: bool,
}

impl Syncback {
	pub fn main(self) -> Result<()> {
		let project_path = project::resolve(self.project.clone().unwrap_or_default())?;

		Config::load_workspace(project_path.get_parent());

		if !project_path.exists() {
			bail!(
				"No project files found in {}",
				project_path.get_parent().to_string().bold()
			);
		}

		if !self.input.exists() {
			bail!("Input file does not exist: {}", self.input.to_string().bold());
		}

		let ext = self.input.get_ext();
		let xml = match ext {
			"rbxlx" | "rbxmx" => true,
			"rbxl" | "rbxm" => false,
			_ => bail!(
				"Invalid input extension: {}. Only {}, {}, {}, {} are allowed",
				ext.bold(),
				"rbxl".bold(),
				"rbxlx".bold(),
				"rbxm".bold(),
				"rbxmx".bold(),
			),
		};

		let project = Project::load(&project_path)?;

		let input_is_place = matches!(ext, "rbxl" | "rbxlx");

		if input_is_place != project.is_place() {
			bail!(
				"Cannot sync back a {} into a {} project",
				if input_is_place { "place" } else { "model" },
				if project.is_place() { "place" } else { "model" },
			);
		}

		let reader = BufReader::new(File::open(&self.input)?);
		let dom = if xml {
			rbx_xml::from_reader_default(reader)?
		} else {
			rbx_binary::from_reader(reader)?
		};

		let core = Core::new(project, false)?;
		let summary = core.syncback(dom, self.prune)?;

		argon_info!(
			"Synced back {} into project: {} ({} added, {} updated, {} removed)",
			self.input.to_string().bold(),
			project_path.to_string().bold(),
			summary.added.to_string().bold().green(),
			summary.updated.to_string().bold().blue(),
			summary.removed.to_string().bold().red(),
		);

		Ok(())
	}
}
