//! How tools start the programs they run: through a launcher the host
//! gives them, such as the repository's direnv environment
//! (`docs/reference/environment.md`), or as they are.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use async_trait::async_trait;

/// What a program starts through: the programs before it, and the
/// variables set over the environment it inherits. The default starts
/// it as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// The launcher's words: the process started is `prefix… program
    /// args…`. Empty starts the program itself.
    pub prefix: Vec<OsString>,
    /// Variables set for the process, over the ones it inherits.
    pub env: Vec<(OsString, OsString)>,
}

impl Launch {
    /// Whether the program starts as it is.
    pub fn is_direct(&self) -> bool {
        self.prefix.is_empty() && self.env.is_empty()
    }

    /// The process to start for `program` with `args`: the program it
    /// executes, then its arguments.
    pub fn argv<I, S>(
        &self,
        program: &OsStr,
        args: I,
    ) -> (OsString, Vec<OsString>)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut words = self
            .prefix
            .iter()
            .cloned()
            .chain(std::iter::once(program.to_owned()))
            .chain(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        let first = words.next().expect("the program is always there");
        (first, words.collect())
    }
}

/// Says how a command in a directory starts. The answer may wait: for
/// the person to say whether the repository's environment loads, or for
/// it to load. Callers bound the wait themselves (a command's timeout,
/// a cancel).
#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, dir: &Path) -> Launch;
}

/// Several launchers, as one: the first one's words come first, and
/// each one's variables after the ones before, so a later one wins.
/// Asks each in turn, so each one's wait counts.
pub struct Launchers(pub Vec<std::sync::Arc<dyn Launcher>>);

#[async_trait]
impl Launcher for Launchers {
    async fn launch(&self, dir: &Path) -> Launch {
        let mut all = Launch::default();
        for launcher in &self.0 {
            let launch = launcher.launch(dir).await;
            all.prefix.extend(launch.prefix);
            all.env.extend(launch.env);
        }
        all
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use super::*;

    /// The process is the prefix, then the program and its arguments, in
    /// order; with no prefix it is the program itself.
    #[hegel::test(test_cases = 200)]
    fn the_process_is_the_prefix_then_the_program(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let word = || gs::text().min_size(1).max_size(8);
        let prefix: Vec<String> = tc.draw(gs::vecs(word()).max_size(4));
        let program: String = tc.draw(word());
        let args: Vec<String> = tc.draw(gs::vecs(word()).max_size(4));
        let launch = Launch {
            prefix: prefix.iter().map(OsString::from).collect(),
            env: Vec::new(),
        };
        let (first, rest) = launch.argv(OsStr::new(&program), &args);
        let mut all = vec![first];
        all.extend(rest);
        let expected: Vec<OsString> = prefix
            .iter()
            .chain(std::iter::once(&program))
            .chain(&args)
            .map(OsString::from)
            .collect();
        assert_eq!(all, expected);
        assert_eq!(launch.is_direct(), prefix.is_empty());
    }

    /// A launcher's words and variables, as drawn.
    type Part = (Vec<String>, Vec<(String, String)>);

    struct Fixed(Launch);

    #[async_trait]
    impl Launcher for Fixed {
        async fn launch(&self, _dir: &Path) -> Launch {
            self.0.clone()
        }
    }

    /// Launchers as one: their words and their variables in their order;
    /// none starts the program as it is.
    #[hegel::test(test_cases = 100)]
    fn launchers_join_in_order(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let word = || gs::text().min_size(1).max_size(6);
        let parts: Vec<Part> = tc.draw(
            gs::vecs(gs::tuples2(
                gs::vecs(word()).max_size(3),
                gs::vecs(gs::tuples2(word(), word())).max_size(3),
            ))
            .max_size(4),
        );
        let launchers = Launchers(
            parts
                .iter()
                .map(|(prefix, env)| {
                    std::sync::Arc::new(Fixed(Launch {
                        prefix: prefix.iter().map(OsString::from).collect(),
                        env: env
                            .iter()
                            .map(|(k, v)| (k.into(), v.into()))
                            .collect(),
                    })) as std::sync::Arc<dyn Launcher>
                })
                .collect(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let launch = runtime.block_on(launchers.launch(Path::new("/")));
        let prefix: Vec<OsString> = parts
            .iter()
            .flat_map(|(prefix, _)| prefix.iter().map(OsString::from))
            .collect();
        let env: Vec<(OsString, OsString)> = parts
            .iter()
            .flat_map(|(_, env)| env.iter().map(|(k, v)| (k.into(), v.into())))
            .collect();
        assert_eq!(launch, Launch { prefix, env });
    }
}
