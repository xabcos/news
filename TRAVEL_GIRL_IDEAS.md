# 旅行女孩 · 产品与存储说明

旅行女孩以固定的成年虚拟人物陪伴用户，结合图片新闻的地点、事件和视觉线索产生旅行动态。已实现的接口和运行配置见 [README.md](README.md)，系统边界见 [DESIGN.md](DESIGN.md)。

## 角色与动态

每个登录账户对应一位旅行女孩。服务端创建身份草稿，用户确认后固定身份快照、身份指纹和初始配饰。新的服装、动作、表情和旅行场景在同一身份上变化。身份以 `travel-girl-v1` 标记，成长阶段由活动记录计算。

生活动态约每四小时更新，结合本地时间、当前旅程、最近动态与公共图片新闻进行创作。女孩的感受属于虚构叙事，新闻资料只提供背景与地点。图片模型通过 `TRAVEL_IMAGE_MODEL` 配置，分辨率和比例由服务端管理。

## 数据边界

所有角色外键统一为 `girl_id`，账户归属通过 `travel_girls.owner_user_id` 唯一关联。

| 表 | 用途 |
| --- | --- |
| `travel_girls` | 固定身份、账户归属、初始配饰与访问时间 |
| `travel_adoption_drafts` | 确认前的身份草稿 |
| `travel_growth_events` | 追加式成长事件与幂等来源 |
| `travel_accessory_unlocks` | 成长配饰 |
| `travel_memories` | 地点、新闻和生活场景记忆 |
| `travel_reading_daily` | 按天聚合的阅读记录 |
| `travel_personality_snapshots` | 性格快照 |
| `travel_world_briefs` | 世界领读与状态 |
| `travel_trips` | 旅程、归来时间、日记与新闻快照 |
| `travel_postcards` | 生成照片、模型、媒体地址及有效期 |
| `travel_encounters` | 共同旅途 |
| `travel_encounter_members` | 共同旅途参与者 |
| `travel_autonomy_state` | 下次活动时间与任务租约 |
| `travel_messages` | 页面展示的生活与旅程记录 |

身份更新由 `protect_travel_girl_identity()` 保护，成长记录由 `forbid_travel_growth_event_rewrite()` 保护。主键、外键、索引、检查约束和触发器统一使用旅行女孩命名。

## 页面与接口

页面位于 `/travel/`，访问 `/api/travel/*`；身份对象返回为 `girl`，旅程的角色字段返回为 `girl_id`。服务端只使用上述表，不保留旧表、旧接口别名或兼容视图。

迁移按版本顺序执行。已执行的迁移文件不可改写，以免 SQLx 校验失败；一次性升级脚本原地修改结构，新角色和旅程记录会被保留。全新数据库执行完整迁移后得到相同的新结构。

## 后续方向

- 验证人物参考图在连续旅行照片中的一致性与媒体有效期。
- 丰富地点、动作、穿搭和镜头组合，结合近期记忆减少重复。
- 完善图片生成的预算统计与任务重试。
- 扩展旅途档案和共同旅程展示。
