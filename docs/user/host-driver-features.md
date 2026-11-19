---
title: Read a host’s driver features
screens: /resources/hosts/$id
keywords: host, driver, features, unavailable, storage, capacity, cluster, ports, images
---

# Read a host’s driver features

## Read the available operations

Open a host’s **Overview**, expand **Details**, and find **Drivers**. Choose
**Features** for a driver. The card lists each reported operation as **Available**
or **Unavailable**, with the reason when it cannot be used. A dash in the Features
column means the host has not reported those facts.

Image builds, image archives, host directories, private views, suspension and
host ports can be unavailable even when the driver can run an environment.
Installed files and transferring installed files are separate operations.
Tool delivery and gateway access describe what a new unit can use.

These are the host’s reported abilities. Access permissions, machine policy and
the chosen template still determine whether a particular request is allowed.
Reading the card starts no work and changes no settings.

## Read capacity and storage

A host is one destination for environments and can represent a pool of several
machines. Its driver chooses the machine for each unit. Capacity and load describe
that offered pool; they do not necessarily describe the computer running the node.

On **Storage → Storages**, a name can describe a disk or a storage class. An
unmeasured capacity or free space appears as a dash. Unknown does not mean empty
or unlimited. Select a storage the host actually offers when configuring new data.
Changing that selection does not move existing data; use the reviewed move flow.

See [named storage](choose-machine-storage.md) and
[environment templates](write-an-environment-template.md) for those choices.
