package dev.claudelauncher.live.mixin;

import java.util.Map;

import net.minecraft.core.MappedRegistry;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

/** Route 4: lets RuntimeRegistrar unfreeze a registry. VERIFY(26.x): field names. */
@Mixin(MappedRegistry.class)
public interface MappedRegistryAccessor {
    @Accessor("frozen")
    void claudeLive$setFrozen(boolean frozen);

    @SuppressWarnings("rawtypes")
    @Accessor("unregisteredIntrusiveHolders")
    Map claudeLive$getIntrusive();

    @SuppressWarnings("rawtypes")
    @Accessor("unregisteredIntrusiveHolders")
    void claudeLive$setIntrusive(Map holders);
}
