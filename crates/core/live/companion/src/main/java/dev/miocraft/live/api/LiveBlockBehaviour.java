package dev.miocraft.live.api;

import net.minecraft.core.BlockPos;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;

/**
 * Behaviour for a live block. Implement this in a mod project, compile it, and
 * point a block definition's {@code behaviour} at the class: it's loaded in a
 * fresh class loader whenever the compiled classes change, so edits apply
 * without restarting. Needs a public no-argument constructor.
 */
public interface LiveBlockBehaviour {
    /** Right-click with an empty hand. */
    default InteractionResult use(BlockState state, Level level, BlockPos pos, Player player, BlockHitResult hit) {
        return InteractionResult.PASS;
    }

    /** An entity walks on the block. */
    default void stepOn(Level level, BlockPos pos, BlockState state, Entity entity) {}

    /** The block was placed (placer may be null). */
    default void placed(Level level, BlockPos pos, BlockState state, LivingEntity placer) {}
}
