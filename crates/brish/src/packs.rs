//! Completion packs: static Rust arrays of subcommands/flags for
//! common CLI tools, config-gated like `default-completion`.
//!
//! Pack name = `pack-<command>` so they don't clash with store
//! plugin names. Enable/disable via `config.toml [plugins]`
//! `enabled = ["brish-pack-git", "brish-pack-docker"]` or
//! `disabled = ["pack-kubectl"]`.

use brish_plugin::{Completion, CompletionCtx, Plugin, Registry};

#[derive(Debug)]
pub struct Pack {
    /// Plugin name (`pack-git`) — unique across catalog + store.
    pub name: &'static str,
    /// The command this pack extends (`git`, `docker`, ...).
    pub command: &'static str,
    /// Subcommands: `(name, description)`. Match when the current word
    /// does NOT start with `-`.
    pub subcommands: &'static [(&'static str, &'static str)],
    /// Flags: `(flag, description)`. Match when the current word
    /// starts with `-`.
    pub flags: &'static [(&'static str, &'static str)],
}

/// One pack, one provider instance.
pub struct PackProvider {
    pack: &'static Pack,
}

impl PackProvider {
    pub fn new(pack: &'static Pack) -> Self {
        Self { pack }
    }
}

impl brish_plugin::CompletionProvider for PackProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if ctx.is_command || ctx.after_dollar {
            return Vec::new();
        }
        // Only fire for *our* command at argument position.
        let before = ctx.line_before.trim();
        let first = before.split_whitespace().next().unwrap_or("");
        if first != self.pack.command {
            return Vec::new();
        }
        let word = ctx.word;
        let mut out = Vec::new();
        let candidates = if word.starts_with('-') {
            self.pack.flags
        } else {
            self.pack.subcommands
        };
        for (val, desc) in candidates {
            if ctx.matches_with(val, Some(desc)) {
                out.push(Completion {
                    value: (*val).to_string(),
                    description: Some((*desc).to_string()),
                    keep_typing: false,
                });
            }
        }
        out
    }
}

/// Plugin wrapper that installs the provider when enabled.
pub struct PackPlugin {
    pack: &'static Pack,
}

impl PackPlugin {
    pub fn new(pack: &'static Pack) -> Self {
        Self { pack }
    }
}

impl Plugin for PackPlugin {
    fn name(&self) -> &str {
        self.pack.name
    }

    fn install(&self, reg: &mut Registry) {
        reg.completion_providers
            .push(Box::new(PackProvider::new(self.pack)));
    }
}

/// Built-in pack catalog. Add new packs here.
pub fn catalog() -> Vec<PackPlugin> {
    vec![
        PackPlugin::new(&GIT_PACK),
        PackPlugin::new(&DOCKER_PACK),
        PackPlugin::new(&CARGO_PACK),
        PackPlugin::new(&KUBECTL_PACK),
        PackPlugin::new(&AWS_PACK),
        PackPlugin::new(&NPM_PACK),
        PackPlugin::new(&MAKE_PACK),
        PackPlugin::new(&MAN_PACK),
    ]
}

/// Pack definitions (command, subcommands, flags). Content is intentionally
/// practical not exhaustive — common daily workflow first.
const GIT_PACK: Pack = Pack {
    name: "brish-pack-git",
    command: "git",
    subcommands: &[
        ("add", "Add file contents to the index"),
        ("branch", "List, create, or delete branches"),
        ("checkout", "Switch branches or restore working tree files"),
        (
            "cherry-pick",
            "Apply the changes introduced by some existing commits",
        ),
        ("clone", "Clone a repository into a new directory"),
        ("commit", "Record changes to the repository"),
        (
            "diff",
            "Show changes between commits, commit and working tree, etc",
        ),
        ("fetch", "Download objects and refs from another repository"),
        ("log", "Show commit logs"),
        ("merge", "Join two or more development histories together"),
        (
            "pull",
            "Fetch from and integrate with another repository or a local branch",
        ),
        ("push", "Update remote refs along with associated objects"),
        ("rebase", "Reapply commits on top of another base tip"),
        ("remote", "Manage set of tracked repositories"),
        ("reset", "Reset current HEAD to the specified state"),
        ("restore", "Restore working tree files"),
        ("show", "Show various types of objects"),
        ("status", "Show the working tree status"),
        (
            "stash",
            "Stash the changes in a dirty working directory away",
        ),
        ("switch", "Switch branches"),
        (
            "tag",
            "Create, list, delete or verify a tag object signed with GPG",
        ),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-C", "Run as if git was started in <path>"),
        ("-c", "Pass configuration parameter"),
        ("--bare", "Treat the repository as bare"),
        ("--git-dir", "Set path to .git directory"),
        ("--work-tree", "Set path to working tree"),
        ("--no-pager", "Do not pipe output into a pager"),
    ],
};

const DOCKER_PACK: Pack = Pack {
    name: "brish-pack-docker",
    command: "docker",
    subcommands: &[
        ("build", "Build an image from a Dockerfile"),
        ("run", "Run a command in a new container"),
        ("ps", "List containers"),
        ("images", "List images"),
        ("pull", "Pull an image or a repository from a registry"),
        ("push", "Push an image or a repository to a registry"),
        ("exec", "Run a command in a running container"),
        ("logs", "Fetch the logs of a container"),
        ("stop", "Stop one or more running containers"),
        ("start", "Start one or more stopped containers"),
        ("restart", "Restart one or more containers"),
        ("rm", "Remove one or more containers"),
        ("rmi", "Remove one or more images"),
        ("network", "Manage networks"),
        ("volume", "Manage volumes"),
        ("compose", "Docker Compose"),
        ("inspect", "Return low-level information on Docker objects"),
        ("top", "Display the running processes of a container"),
        (
            "stats",
            "Display a live stream of container(s) resource usage",
        ),
        (
            "attach",
            "Attach local standard input, output, and error streams to a running container",
        ),
        ("kill", "Kill one or more running containers"),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-H", "Daemon socket to connect to"),
        ("--config", "Location of client config files"),
        ("-D", "Enable debug mode"),
    ],
};

const CARGO_PACK: Pack = Pack {
    name: "brish-pack-cargo",
    command: "cargo",
    subcommands: &[
        ("build", "Compile the current package"),
        ("check", "Analyze the current package and report errors"),
        ("run", "Run the main binary of the current package"),
        ("test", "Run the tests"),
        ("doc", "Build documentation"),
        ("clean", "Remove the target directory"),
        ("update", "Update dependencies as recorded in Cargo.lock"),
        ("add", "Add a dependency to Cargo.toml"),
        ("remove", "Remove a dependency from Cargo.toml"),
        ("tree", "Display the dependency tree"),
        ("publish", "Publish a crate to crates.io"),
        ("search", "Search crates on crates.io"),
        (
            "init",
            "Create a new cargo package in an existing directory",
        ),
        ("new", "Create a new cargo package"),
        ("install", "Install a binary crate"),
        ("uninstall", "Uninstall a binary crate"),
        ("search", "Search crates on crates.io"),
        ("metadata", "Output metadata about the package"),
        ("tree", "Display the dependency tree"),
        ("bench", "Run benchmarks"),
        ("fmt", "Format all manifest files"),
        ("clippy", "Lint checks via clippy"),
        ("fix", "Automatically fix lint warnings"),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("--verbose", "Use verbose output"),
        ("--quiet", "No output printed to stdout"),
        ("--color", "Coloring: auto, always, never"),
        ("--frozen", "Require Cargo.lock up to date"),
        ("--locked", "Require Cargo.lock up to date"),
        ("--offline", "Run without accessing the network"),
        ("-j", "Number of parallel jobs"),
        ("--target", "Build for the target triple"),
        ("--release", "Build artifacts in release mode"),
        ("--profile", "Build with the given profile"),
    ],
};

const KUBECTL_PACK: Pack = Pack {
    name: "brish-pack-kubectl",
    command: "kubectl",
    subcommands: &[
        ("get", "Display one or many resources"),
        ("describe", "Show details of a specific resource or group"),
        (
            "apply",
            "Apply a configuration to a resource by file or stdin",
        ),
        (
            "delete",
            "Delete resources by file, stdin, or specifying resource names",
        ),
        ("logs", "Print the logs for a container in a pod"),
        ("exec", "Execute a command in a container"),
        ("describe", "Show details of a specific resource"),
        ("expose", "Expose a resource as a new Kubernetes Service"),
        ("scale", "Set a new size for a Deployment, ReplicaSet, etc."),
        ("rollout", "Manage the rollout of a resource"),
        ("config", "Modify kubeconfig files"),
        ("cluster-info", "Display cluster info"),
        ("top", "Display resource (CPU/memory) usage"),
        ("cordon", "Mark node as unschedulable"),
        ("drain", "Drain node in preparation for maintenance"),
        ("label", "Update the labels on a resource"),
        ("annotate", "Update the annotations on a resource"),
        ("diff", "Diff live version against would-be applied version"),
        (
            "wait",
            "Experimental: Wait for a specific condition on one or many resources",
        ),
        ("version", "Print the client and server version information"),
        (
            "api-resources",
            "Print the supported API resources on the server",
        ),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-n", "Namespace scope for this CLI request"),
        (
            "--all-namespaces",
            "If present, list the requested object(s) across all namespaces",
        ),
        ("-o", "Output format: json, yaml, name, etc"),
        ("-l", "Selector (label query) to filter on"),
        ("--field-selector", "Selector (field query) to filter on"),
        ("--context", "The name of the kubeconfig context to use"),
        ("--kubeconfig", "Path to the kubeconfig file to use"),
        (
            "--server",
            "The address and port of the Kubernetes API server",
        ),
    ],
};

const AWS_PACK: Pack = Pack {
    name: "brish-pack-aws",
    command: "aws",
    subcommands: &[
        ("s3", "Manage S3 buckets and objects"),
        ("ec2", "Manage EC2 instances"),
        ("iam", "Manage IAM users, roles, policies"),
        ("sts", "Security Token Service"),
        ("lambda", "Manage Lambda functions"),
        ("dynamodb", "Manage DynamoDB tables"),
        ("sns", "Manage SNS topics and subscriptions"),
        ("sqs", "Manage SQS queues"),
        ("cloudformation", "Manage CloudFormation stacks"),
        ("ecs", "Manage ECS clusters and services"),
        ("ecr", "Manage ECR repositories"),
        ("rds", "Manage RDS instances"),
        ("cloudwatch", "Manage CloudWatch alarms, logs, metrics"),
        ("configure", "Configure AWS CLI settings"),
        ("logs", "Manage CloudWatch Logs"),
        ("apigateway", "Manage API Gateway"),
        ("route53", "Manage Route 53 hosted zones"),
        ("efs", "Manage EFS file systems"),
        ("ssm", "Manage Systems Manager"),
        ("secretsmanager", "Manage Secrets Manager secrets"),
        ("sso", "Manage AWS Single Sign-On"),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("--region", "AWS region to use"),
        ("--profile", "Use a specific profile from credential file"),
        (
            "--output",
            "Output format: json, text, table, yaml, yaml-stream",
        ),
        ("--query", "JMESPath query to filter output"),
        ("--no-cli-pager", "Disable CLI pager for output"),
        ("--no-verify-ssl", "Do not verify SSL certificates"),
        ("--endpoint-url", "Override service endpoint URL"),
    ],
};

const NPM_PACK: Pack = Pack {
    name: "brish-pack-npm",
    command: "npm",
    subcommands: &[
        ("install", "Install dependencies"),
        ("ci", "Install project with clean slate"),
        ("run", "Run arbitrary package scripts"),
        ("test", "Run test script"),
        ("start", "Run start script"),
        ("build", "Run build script"),
        ("publish", "Publish a package"),
        ("pack", "Create a tarball from a package"),
        ("view", "View package info"),
        ("search", "Search for packages"),
        ("init", "Create a package.json file"),
        ("uninstall", "Remove a package"),
        ("update", "Update dependencies"),
        ("audit", "Run security audit"),
        ("fund", "Retrieve funding information"),
        ("outdated", "Check for outdated packages"),
        ("exec", "Run a command from a package"),
        ("ls", "List installed packages"),
        ("link", "Symlink a package folder"),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-g", "Global install"),
        ("-D", "Save as devDependency"),
        ("-S", "Save as dependency"),
        ("--save", "Save installed packages to dependencies"),
        ("--save-dev", "Save as devDependency"),
        ("--save-optional", "Save as optionalDependency"),
        ("--production", "Install production dependencies only"),
        ("--prefer-offline", "Skip network requests"),
    ],
};

const MAKE_PACK: Pack = Pack {
    name: "brish-pack-make",
    command: "make",
    subcommands: &[], // make targets are project-specific; flags only
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-C", "Change to directory before reading makefile"),
        ("-f", "Use FILE as makefile"),
        ("-j", "Number of jobs to run simultaneously"),
        ("-k", "Continue as much as possible after an error"),
        ("-n", "Dry run (print commands but don't execute)"),
        ("-s", "Silent operation"),
        ("-q", "Question mode (exit 0 if targets up to date)"),
        ("-w", "Print working directory"),
        ("-e", "Environment variables override makefile"),
        ("-i", "Ignore errors from commands"),
        ("--directory", "Same as -C"),
        ("--file", "Same as -f"),
        ("--jobs", "Same as -j"),
        ("--keep-going", "Same as -k"),
        ("--dry-run", "Same as -n"),
        ("--silent", "Same as -s"),
        ("--question", "Same as -q"),
    ],
};

const MAN_PACK: Pack = Pack {
    name: "brish-pack-man",
    command: "man",
    subcommands: &[
        ("1", "User commands"),
        ("2", "System calls"),
        ("3", "Library functions"),
        ("4", "Special files"),
        ("5", "File formats and conventions"),
        ("6", "Games and screensavers"),
        ("7", "Miscellaneous"),
        ("8", "System administration commands"),
        ("9", "Kernel routines"),
    ],
    flags: &[
        ("--help", "Show help"),
        ("--version", "Show version"),
        ("-a", "Display all manual pages"),
        ("-f", "Equivalent to whatis"),
        ("-k", "Equivalent to apropos"),
        ("-t", "Format as troff"),
        ("-w", "Show location only"),
        ("-P", "Use pager"),
        ("-s", "Section list"),
        ("-S", "Same as -s"),
        ("--locale", "Locale"),
        ("--encoding", "Output encoding"),
        ("-H", "HTML output"),
        ("-l", "Local file"),
    ],
};
