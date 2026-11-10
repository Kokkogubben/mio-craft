package dev.claudelauncher.live.api;

import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.Level;

/** Behaviour for a live item; see {@link LiveBlockBehaviour} for how it's loaded. */
public interface LiveItemBehaviour {
    /** Right-click while holding the item. */
    default InteractionResult use(Level level, Player player, InteractionHand hand) {
        return InteractionResult.PASS;
    }
}
