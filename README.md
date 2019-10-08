# Readme
Warning Counter (wcnt) is a small command line utility to count the number of warnings in log files, and compare them to
defined limits.

The kinds of warnings are defined in `Wcnt.toml` which should be located at the root of your project.
Limits are defined in `Limits.toml` which are then valid for source files in that tree of directories
on the file system. You can have multiple `Limits.toml` files and place them where you see fit. Perhaps one
per component, or subsystem, whichever fits your project the best. If the system does not find a `Limits.toml` file
when searching for a set limit, it will use 0 for a limit, so be sure you specify your limits!

## Example Wcnt.toml
Below follows an example `Wcnt.toml` file, defining rules for the two kinds `gcc` and `flake8`.
```toml
[gcc]
regex = "^(?P<file>[^:]+):(?P<line>\\d+):(?P<column>\\d+): warning: (?P<description>.+) \\[(?P<category>.+)\\]"
files = ["**/compilation.log", "**/build.log"]

[flake8]
regex = "^(?P<file>[^:]+):(?P<line>\\d+):(?P<column>\\d+): (?P<category>[^\\s]+) (?P<description>.+)$"
files = ["**/lint.log"]
```

Inside `Wcnt.toml`, you define a map for each "kind" of warning you want to search for, and how to search for it.
Required settings for each are `regex` and `files`. The `regex` value *must* define a `file` capture group, so we know
which file was responsible for each particular warning, and thus, which `Limits.toml` should be used. 

The capture groups `line`, `column`, `category` and `description` are optional and allows the system to disregard
multiples of the same warning. The `category` key also allows you to define individual limits for different categories.

In order to be able to use per catgory limits, your regex *must* define a `category` capture group. Otherwise the system
will abort when parsing the `Limits.toml` file. This is to prevent a false sense of security.

## Example Limits.toml
Below follows an example `Limits.toml` file where `flake8` warnings are capped at 300, and `gcc` warnings are separated
into a few different categories. You can use `inf` to allow any number of warnings, and `_` is the wildcard category.
It matches any category you have not already defined. When using per-category limits, it's always wise to include the
wildcard category, otherwise the limit is zero.

```toml
flake8 = 300 

[gcc]
-Wpedantic = 3
-Wcomment = inf
-Wunused-variable = 2 
_ = 0
```
When not using per-category limits, all categories are counted towards the same limit. In other words, these two ways
of defining limits are equivalent.
```toml
kind = 1
```
and
```toml
[kind]
_ = 1
```

### You can have multiple `Limits.toml` files
Every warning from a source file are counted towards the `Limits.toml` file that are closest to it going straight up
file system tree. In the example below, `component_a` and `component_b` share the limits defined in
`project/src/Limit.toml` while `component_c` has its own limits. This is useful if you have some component that
should have extra strict, or extra loose rules. Such as a newly developed piece of code, or a [vendored](https://stackoverflow.com/questions/35109393/what-does-vendoring-mean-in-go)
third party dependency or legacy code.
```plain
project
├── Wcnt.toml
├── src
│   ├── Limits.toml
│   ├── component_a
│   │   ├── source.c
│   │   ├── interface.h
│   │   └── utility.py
│   ├── component_b
│   │   ├── binary.c
│   │   ├── code.c
│   │   └── interface.h
│   └── component_c
│       ├── Limits.toml
│       ├── rustmodule.rs
│       └── glue.c
└── build
    ├── compilation.log
    └── lint.log
```

## Pruning
The tool can automatically update/lower and prune your `Limits.toml` files.
When you have zero warnings, with the flag `--update-limits` the following limits:
```toml
[gcc]
-Wpedantic = 3
-Wcomment = 3
-Wunused-variable = 2
```
turns into
```toml
[gcc]
-Wpedantic = 0
-Wcomment = 0
-Wunused-variable = 0
```
*Note*: `--update-limits` does not touch limits set to `inf`.

With the addition of `--prune`, the above limits are reduced to
```toml
gcc = 0
```
*Note*: `--prune` also does not touch limits set to `inf`.

It is strongly recommended to have a automated recurring task which runs `wcnt --update-limits [--prune]` and commits
the results into your repository, so you can ensure that the limits are indeed lowered over time.

## Output from --help
```plain
$ wcnt --help
Warning Counter (wcnt) 0.2.0
Mikael Silvén <mikael@silven.nu>
A program to count your warnings inside log files and comparing them against defined limits.

USAGE:
    wcnt.exe [FLAGS] [OPTIONS]

FLAGS:
    -h, --help             Prints help information
        --prune            Also aggressively prune Limits.toml files to more minimal forms (requires --update-limits).
        --update-limits    Update the Limit.toml files with lower values if no violations were found.
    -V, --version          Prints version information
    -v                     Be more verbose. (Add more for more)

OPTIONS:
        --config <Wcnt.toml>    Use this config file. (Instead of <start>/Wcnt.toml)
        --start <DIR>           Start search in this directory (instead of cwd)
```

