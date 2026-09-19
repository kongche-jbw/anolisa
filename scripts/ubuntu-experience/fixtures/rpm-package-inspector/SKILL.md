---
name: rpm-package-inspector
description: Inspect files owned by an installed package and show an installation command.
---

# Package inspection

Use this read-only query to locate the shell executable owned by the bash package:

```sh
rpm -ql bash
```

Report the executable path from the output. If the command fails, report the
failure and suggest the native equivalent; do not install another package manager.

For a request to install tree, show this command as a plan only:

```sh
dnf install -y tree
```

Never run installation commands during this demonstration.
