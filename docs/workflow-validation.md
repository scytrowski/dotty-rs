# GitHub Actions workflow validation

Run the same static workflow checks used by CI with:

```sh
tools/check-workflows.sh
```

The script uses `actionlint` 1.7.12 from `PATH` when available. If it is
missing or a different version is installed, it uses the pinned official
`rhysd/actionlint:1.7.12` Docker image. To install the binary with Go instead,
run:

```sh
go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12
```

The main Rust CI workflow runs this script on every pull request and push to
`main`. It checks all workflow files for YAML and workflow-schema errors,
invalid expression contexts, invalid action inputs, and related mistakes.
