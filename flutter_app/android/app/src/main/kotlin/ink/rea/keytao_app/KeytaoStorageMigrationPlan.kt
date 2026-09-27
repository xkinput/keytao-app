package ink.rea.keytao_app

import java.io.File

internal data class StorageRootSnapshot(
    val root: File,
    val hasContent: Boolean,
    val modifiedAt: Long,
    val hasUserDataOrSchema: Boolean = false,
)
internal data class StorageMigrationDecision(val source: File? = null, val renameExisting: Boolean = false)

/** Pure policy: data/schema roots win; then compare data timestamps, keeping input order on ties. */
internal fun decideStorageMigration(
    sharedHasContent: Boolean,
    oldRoots: List<StorageRootSnapshot>,
): StorageMigrationDecision {
    val source = oldRoots.filter { it.hasContent }.maxWithOrNull(
        compareBy<StorageRootSnapshot> { it.hasUserDataOrSchema }.thenBy { it.modifiedAt },
    )?.root
    return StorageMigrationDecision(source, source != null && sharedHasContent)
}
