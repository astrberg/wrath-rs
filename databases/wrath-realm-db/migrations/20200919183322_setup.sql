CREATE TABLE `characters` (
`id` int(10) unsigned  NOT NULL AUTO_INCREMENT,
`account_id` int(10) unsigned NOT NULL DEFAULT '0',
`name` varchar(25) NOT NULL DEFAULT '',
`race` tinyint(3) unsigned NOT NULL DEFAULT '0',
`class` tinyint(3) unsigned NOT NULL DEFAULT '0',
`gender` tinyint(3) unsigned NOT NULL DEFAULT '0',
`skin_color` tinyint(3) unsigned NOT NULL DEFAULT '0',
`face` tinyint(3) unsigned NOT NULL DEFAULT '0',
`hair_style` tinyint(3) unsigned NOT NULL DEFAULT '0',
`hair_color` tinyint(3) unsigned NOT NULL DEFAULT '0',
`facial_style` tinyint(3) unsigned NOT NULL DEFAULT '0',
`player_flags` int(10) unsigned NOT NULL DEFAULT '0',
`at_login_flags` smallint(5) unsigned NOT NULL DEFAULT '0',
`zone` smallint(5) unsigned NOT NULL DEFAULT '0',
`level` tinyint(3) unsigned NOT NULL DEFAULT '1',
`map` smallint(5) unsigned NOT NULL DEFAULT '0',
`x` float NOT NULL DEFAULT '0',
`y` float NOT NULL DEFAULT '0',
`z` float NOT NULL DEFAULT '0',
`o` float NOT NULL DEFAULT '0',
`instance_id` int (10) unsigned NOT NULL DEFAULT '0',
`bind_zone` smallint(5) unsigned NOT NULL DEFAULT '0',
`bind_map` smallint(5) unsigned NOT NULL DEFAULT '0',
`bind_x` float NOT NULL DEFAULT '0',
`bind_y` float NOT NULL DEFAULT '0',
`bind_z` float NOT NULL DEFAULT '0',
`guild_id` int(10) unsigned NOT NULL DEFAULT '0',
`tutorial_data` BINARY(32) NOT NULL,
`playtime_total` int(10) unsigned NOT NULL DEFAULT '0',
`playtime_level` int(10) unsigned NOT NULL DEFAULT '0',
PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `character_account_data` (
`character_id` int(10) unsigned NOT NULL DEFAULT '0',
`data_type` tinyint unsigned NOT NULL DEFAULT '0',
`time` bigint unsigned NOT NULL DEFAULT '0',
`decompressed_size` int unsigned NOT NULL DEFAULT '0',
`data` longblob,
KEY `FK_CHARACTER_ACCOUNT_DATA_CHARACTER` (`character_id`),
CONSTRAINT `FK_CHARACTER_ACCOUNT_DATA_CHARACTER` FOREIGN KEY (`character_id`) REFERENCES `characters` (`id`) ON DELETE CASCADE ON UPDATE RESTRICT,
PRIMARY KEY (`character_id`, `data_type`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci;

CREATE TABLE `character_equipment` (
  `character_id` int(10) unsigned NOT NULL DEFAULT '0' COMMENT 'The ID of the character.',
  `slot_id` tinyint(2) unsigned NOT NULL DEFAULT '0' COMMENT 'Equipped Slot index.',
  `item` int(10) unsigned DEFAULT NULL COMMENT 'Equipped item',
  `enchant` int(10) unsigned DEFAULT NULL COMMENT 'Enchantment',
  CONSTRAINT `FK_CHARACTER_EQUIPMENT_CHARACTER` FOREIGN KEY (`character_id`) REFERENCES `characters` (`id`) ON DELETE CASCADE ON UPDATE RESTRICT,
  CONSTRAINT `FK_CHARACTER_EQUIPMENT_ITEM` FOREIGN KEY (`item`) REFERENCES wrath_game.item_template (`id`) ON DELETE RESTRICT ON UPDATE CASCADE,
  PRIMARY KEY (`character_id`, `slot_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8 ROW_FORMAT=DYNAMIC COMMENT='Trigger System';
