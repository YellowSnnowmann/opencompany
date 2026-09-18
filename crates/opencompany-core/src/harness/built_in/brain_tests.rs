use super::*;
use crate::ports::TaskStore;
use crate::ports::tasks::TaskTitle;
use std::sync::Mutex as StdMutex;
use tinyinference::model::ModelRequest;

#[path = "brain_tests_support1.rs"]
mod support1;
use support1::*;
#[path = "brain_tests_support2.rs"]
mod support2;
use support2::*;
#[path = "brain_tests_support3.rs"]
mod support3;
use support3::*;
#[path = "brain_tests_support4.rs"]
mod support4;
use support4::*;
#[path = "brain_tests_support5.rs"]
mod support5;
use support5::*;

#[path = "brain_tests_part1.rs"]
mod tests_part1;
#[path = "brain_tests_part10.rs"]
mod tests_part10;
#[path = "brain_tests_part11.rs"]
mod tests_part11;
#[path = "brain_tests_part2.rs"]
mod tests_part2;
#[path = "brain_tests_part3.rs"]
mod tests_part3;
#[path = "brain_tests_part4.rs"]
mod tests_part4;
#[path = "brain_tests_part5.rs"]
mod tests_part5;
#[path = "brain_tests_part6.rs"]
mod tests_part6;
#[path = "brain_tests_part7.rs"]
mod tests_part7;
#[path = "brain_tests_part8.rs"]
mod tests_part8;
#[path = "brain_tests_part9.rs"]
mod tests_part9;
