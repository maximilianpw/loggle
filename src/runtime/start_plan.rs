use std::{collections::BTreeMap, fmt};

use super::StartCommand;

#[derive(Debug, Clone)]
pub(crate) struct StartPlan<'a> {
    commands: &'a [StartCommand],
    command_indexes: BTreeMap<&'a str, usize>,
}

impl<'a> StartPlan<'a> {
    pub(crate) fn new(commands: &'a [StartCommand]) -> Result<Self, StartPlanError> {
        let command_indexes = commands
            .iter()
            .enumerate()
            .map(|(index, command)| (command.name.as_str(), index))
            .collect::<BTreeMap<_, _>>();

        let plan = Self {
            commands,
            command_indexes,
        };
        plan.validate_dependencies()?;

        Ok(plan)
    }

    pub(crate) fn len(&self) -> usize {
        self.commands.len()
    }

    pub(crate) fn command(&self, index: usize) -> &StartCommand {
        &self.commands[index]
    }

    pub(crate) fn dependency_indexes(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.commands[index]
            .wait_for
            .iter()
            .map(|dependency| self.command_indexes[dependency.as_str()])
    }

    fn validate_dependencies(&self) -> Result<(), StartPlanError> {
        for command in self.commands {
            for dependency in &command.wait_for {
                if !self.command_indexes.contains_key(dependency.as_str()) {
                    return Err(StartPlanError::UnknownDependency {
                        command: command.name.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        let mut states = vec![VisitState::Unvisited; self.commands.len()];
        let mut stack = Vec::new();
        for index in 0..self.commands.len() {
            self.visit_dependencies(index, &mut states, &mut stack)?;
        }

        Ok(())
    }

    fn visit_dependencies(
        &self,
        index: usize,
        states: &mut [VisitState],
        stack: &mut Vec<usize>,
    ) -> Result<(), StartPlanError> {
        match states[index] {
            VisitState::Visited => return Ok(()),
            VisitState::Visiting => {
                return Err(StartPlanError::DependencyCycle {
                    cycle: self.cycle_names(stack, index),
                });
            }
            VisitState::Unvisited => {}
        }

        states[index] = VisitState::Visiting;
        stack.push(index);

        for dependency_index in self.dependency_indexes(index) {
            self.visit_dependencies(dependency_index, states, stack)?;
        }

        stack.pop();
        states[index] = VisitState::Visited;
        Ok(())
    }

    fn cycle_names(&self, stack: &[usize], repeated_index: usize) -> Vec<String> {
        let start = stack
            .iter()
            .position(|index| *index == repeated_index)
            .unwrap_or(0);
        let mut names = stack[start..]
            .iter()
            .map(|index| self.commands[*index].name.clone())
            .collect::<Vec<_>>();
        names.push(self.commands[repeated_index].name.clone());

        names
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartPlanError {
    UnknownDependency { command: String, dependency: String },
    DependencyCycle { cycle: Vec<String> },
}

impl fmt::Display for StartPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDependency {
                command,
                dependency,
            } => write!(
                f,
                "command '{command}' waits for unknown command '{dependency}'"
            ),
            Self::DependencyCycle { cycle } => {
                write!(
                    f,
                    "command dependency cycle detected: {}",
                    cycle.join(" -> ")
                )
            }
        }
    }
}

impl std::error::Error for StartPlanError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn command(name: &str, wait_for: &[&str]) -> StartCommand {
        StartCommand {
            name: name.to_string(),
            argv: vec!["true".to_string()],
            cwd: Some(PathBuf::from("/tmp")),
            env: BTreeMap::new(),
            wait_for: wait_for
                .iter()
                .map(|dependency| dependency.to_string())
                .collect(),
            ready: None,
        }
    }

    #[test]
    fn exposes_dependency_indexes_for_valid_plan() {
        let commands = vec![command("db", &[]), command("api", &["db"])];
        let plan = StartPlan::new(&commands).unwrap();

        assert_eq!(plan.dependency_indexes(1).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn rejects_unknown_dependencies() {
        let commands = vec![command("api", &["db"])];
        let error = StartPlan::new(&commands).unwrap_err();

        assert_eq!(
            error.to_string(),
            "command 'api' waits for unknown command 'db'"
        );
    }

    #[test]
    fn rejects_dependency_cycles() {
        let commands = vec![command("api", &["web"]), command("web", &["api"])];
        let error = StartPlan::new(&commands).unwrap_err();

        assert_eq!(
            error.to_string(),
            "command dependency cycle detected: api -> web -> api"
        );
    }
}
