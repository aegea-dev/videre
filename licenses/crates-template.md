## Rust crates

The open-source Rust crates compiled into this build. Each license is
followed by the crates that use it.

{{#each licenses}}
### {{{name}}}

Used by:
{{#each used_by}}
- [{{{crate.name}}} {{{crate.version}}}]({{#if crate.repository}}{{{crate.repository}}}{{else}}https://crates.io/crates/{{{crate.name}}}{{/if}})
{{/each}}

```
{{{text}}}
```

{{/each}}
