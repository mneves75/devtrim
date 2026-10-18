# devtrim

devtrim measures developer-machine disk usage and previews narrowly authorized
cleanup. A disk inventory and permission to remove an item are separate concepts.

## Language

**Disk inventory**:
A read-only description of the files and directories visible within a selected
folder. Presence in the inventory says nothing about whether removal is appropriate.
_Avoid_: cleanup plan, safe files

**Mapped bytes**:
Estimated logical file lengths represented by the disk inventory. They are not
exclusive physical blocks or a promise of immediately available space.
_Avoid_: disk space reclaimed, physical usage

**Available disk space**:
The operating system's reported space available on the selected folder's
filesystem. It is independent of the sum of mapped logical bytes.
_Avoid_: unmapped bytes

**Cleanup preview**:
Findings produced by devtrim's existing cleanup categories under their ownership,
activity and protection rules. A fresh preview is required before an apply.
_Avoid_: safe to remove, automatic cleanup

**Dashboard snapshot**:
A local, read-only disk inventory and cleanup-preview report captured at a
particular time. It becomes historical as the filesystem changes.
_Avoid_: live disk state

**Partial measurement**:
A lower bound whose traversal or metadata could not be read completely. Missing
information remains unknown rather than being counted as zero.
_Avoid_: complete usage
