# Readme
Warning Counter (wcnt) is a small command line utility to count the number of warnings in log files, and compare them to
defined limits. Useful in CI environments where you want to ensure the number or warnings does not increase.

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
multiples of the same warning (Useful for header files). 
The `category` key also allows you to define individual limits for different categories.

In order to be able to use per-category limits, your regex *must* define a `category` capture group. Otherwise the system
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
*Note*: Per-category definitions must be at the end of file, because of [how TOML works](https://github.com/alexcrichton/toml-rs/issues/142).

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

### Infinite limits
If you've got a particular kind of warning that you do not want to bother with for a certain part of the code base, you
can specify the limit to be `inf`, like so:
```toml
[kind]
-Wbothersome = inf
```
This is useful if you've got vendored code, or experimental code, which you do not want or can keep to the same standard
as your production code, but still want to compile with otherwise the exact same settings.

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

## Partial runs
In some circumstances, you don't want to (or can't) have all warnings available at once. For example if you compile
your C code using both GCC and MSVC/XCode. Then you can pass arguments using the  `--only` flag, to run the tool for
only that or those kinds of warnings. This functionality integrates with `--update-limits` and does not remove
limits from your `Limits.toml` files. If you have recurring jobs automatically making commits to lower your limits,
you will have to take care of any merge conflicts yourself.

## Output from --help
```plain
$ wcnt --help
Warning Counter (wcnt) 0.3.0
Mikael Silvén <mikael@silven.nu>
A program to count your warnings inside log files and comparing them against defined limits.

USAGE:
    wcnt.exe [FLAGS] [OPTIONS]

FLAGS:
    -h, --help             Prints help information
    -V, --version          Prints version information
        --all              Also print non-violating warnings. (if verbose or very very verbose)
        --update-limits    Update the Limit.toml files with lower values if no violations were found.
        --prune            Also aggressively prune Limits.toml files to more minimal forms (requires --update-limits).
    -v                     Be more verbose. (Add more for more)

OPTIONS:
        --only <KIND>...        Run the check only for these kinds of warnings.
        --start <DIR>           Start search in this directory (instead of cwd)
        --config <Wcnt.toml>    Use this config file. (Instead of <start>/Wcnt.toml)
```

## Design goals
* Wcnt tries to not do too many things.
* It does try to be flexible, so your build system doesn't have to be.
* Wcnt should not give a false sense of security.

### Open issues
* I have not yet decided how to handle warnings that originate from outside your codebase. 
Hopefully you can use `-isystem` for these things.
* Windows paths are bothersome, and if your tool outputs `\\?\`-style paths you might be in trouble. 
* I'd like to have a "remapping" feature, so you can analyse your warnings even if they use absolute paths, and where
generated on a different system than where you analyze them.