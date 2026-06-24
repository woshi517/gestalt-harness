# ADR-001 Extension Package Components

Status: Accepted

Gestalt extensions are modeled as packages containing typed components. A configured instance binds a package to host configuration, while process instances and MCP clients are runtime-owned execution resources.

This separates runtime modules, packages, components, configured instances, process instances, runtime generations, and client/product descriptors. Client/product descriptors are inventory for embedding hosts and are not executed by the runtime.

Deferred: sandbox implementation, package registries, dependency lockfiles, remote extension transport, and client code loading.
