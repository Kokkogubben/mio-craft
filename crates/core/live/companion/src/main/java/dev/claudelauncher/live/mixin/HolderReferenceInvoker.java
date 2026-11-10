package dev.claudelauncher.live.mixin;

import java.util.Collection;

import net.minecraft.core.Holder;
import net.minecraft.tags.TagKey;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Invoker;

/** Route 4: new holders need (empty) tags bound. VERIFY(26.x): method name and parameter type. */
@Mixin(Holder.Reference.class)
public interface HolderReferenceInvoker {
    @SuppressWarnings("rawtypes")
    @Invoker("bindTags")
    void claudeLive$bindTags(Collection tags);
}
