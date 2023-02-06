# Database Maintenance

`nostr-rs-relay` uses the SQLite embedded database to minimize
dependencies and overall footprint of running a relay.  If traffic is
light, the relay should just run with very little need for
intervention.  For heavily trafficked relays, there are a number of
steps that the operator may need to take to maintain performance and
limit disk usage.

This maintenance guide is current as of version `0.8.2`.  Future
versions may incorporate and automate some of these steps.

## Backing Up the Database

To prevent data loss, the database should be backed up regularly.  The
recommended method is to use the `sqlite3` command to perform an
"Online Backup".  This can be done while the relay is running, queries
can still run and events will be persisted during the backup.

The following commands will perform a backup of the database to a
dated file, and then compress to minimize size:

```console
BACKUP_FILE=/var/backups/nostr/`date +%Y%m%d_%H%M`.db
sqlite3 -readonly /apps/nostr-relay/nostr.db ".backup $BACKUP_FILE"
sqlite3 $BACKUP_FILE "vacuum;"
bzip2 -9 $BACKUP_FILE
```

Nostr events are very compressible.  Expect a compression ratio on the
order of 4:1, resulting in a 75% space saving.

## Vacuuming the Database

As the database is updated, it can become fragmented.  Performing a
full `vacuum` will rebuild the entire database file, and can reduce
space.  Running this may reduce the size of the database file,
especially if a large amount of data was updated or deleted.

```console
vacuum;
```

## Clearing Hidden Events

When events are deleted, the event is not actually removed from the
database.  Instead, a flag `HIDDEN` is set to true for the event,
which excludes it from search results.  High volume replacements from
profile or other replaceable events are deleted, not hidden, in the
current version of the relay.

In the current version, removing hidden events should not result in
significant space savings, but it can still be used if there is no
desire to hold on to events that can never be re-broadcast.

```console
PRAGMA foreign_keys = ON;
delete from event where HIDDEN=true;
```

## Manually Removing Events

For a variety of reasons, an operator may wish to remove some events
from the database.  The only way of achieving this today is with
manually run SQL commands.

It is recommended to have a good backup prior to manually running SQL
commands!

In all cases, it is mandatory to enable foreign keys, and this must be
done for every connection.  Otherwise, you will likely orphan rows in
the `tag` table.

### Deleting Specific Event

```console
PRAGMA foreign_keys = ON;
delete from event where event_hash=x'00000000000c1271675dc86e3e1dd1336827bccabb90dc4c9d3b4465efefe00e';
```

### Deleting All Events for Pubkey

```console
PRAGMA foreign_keys = ON;
delete from event where author=x'000000000002c7831d9c5a99f183afc2813a6f69a16edda7f6fc0ed8110566e6';
```

### Deleting All Events of a Kind


```console
PRAGMA foreign_keys = ON;
delete from event where kind=70202;
```

### Deleting Old Events

In this scenario, we wish to delete any event that has been stored by
our relay for more than 1 month.  Crucially, this is based on when the
event was stored, not when the event says it was created.  If an event
has a `created` field of 2 years ago, but was first sent to our relay
yesterday, it would not be deleted in this scenario.  Keep in mind, we
do not track anything for re-broadcast events that we already have, so
this is not a very effective way of implementing a "least recently
seen" policy.

```console
PRAGMA foreign_keys = ON;
TODO!
```

### Delete Profile Events with No Recent Events

Many users create profiles, post a "hello world" event, and then never
appear again (likely using an ephemeral keypair that was lost in the
browser cache).  We can find these accounts and remove them after some
time.

```console
PRAGMA foreign_keys = ON;
TODO!
```
