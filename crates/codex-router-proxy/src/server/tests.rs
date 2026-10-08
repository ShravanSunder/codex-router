use super::*;
use tests_part1::*;
use tests_part2::*;
use tests_part3::*;

#[path = "tests_part1.rs"]
mod tests_part1;
#[path = "tests_part2.rs"]
mod tests_part2;
#[path = "tests_part3.rs"]
mod tests_part3;
#[path = "tests_runtime_shutdown.rs"]
mod tests_runtime_shutdown;
#[path = "tests_stopped_serving.rs"]
mod tests_stopped_serving;

#[path = "tests_actor_drain.rs"]
mod tests_actor_drain;

#[path = "tests_actor_join_error.rs"]
mod tests_actor_join_error;
