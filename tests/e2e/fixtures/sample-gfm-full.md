# GFM 全量元素渲染测试规格 (H1)

## 二级标题：排版、强调与分割线 (H2)

### 三级标题 (H3)

#### 四级标题 (H4)

##### 五级标题 (H5)

###### 六级标题 (H6)

正文段落包含 **加粗**、*斜体*、~~删除线~~、`cargo build --release` 行内代码，以及中文标点与英文单词 mixed spacing 验证。

---

## 任务列表与多级嵌套列表

- [x] 已完成：完成技术选型（内存 > UI 美化 > 数据安全）
- [ ] 待办：验证编辑保存自动切回阅读模式
- 顶级无序项
  - 二级嵌套项
    - 三级嵌套项
      - 四级嵌套项
        - 五级嵌套项
1. 第一步：启动应用
   1. 子步骤 1.1
   2. 子步骤 1.2
2. 第二步：检查排版

## 多级引用块

> 第一层引用：纯粹的桌面端 Markdown 阅读器。
>
> > 第二层嵌套引用：界面极简，配置收口于单一 `config.json`。

## GFM 表格渲染

| 模块名 | 对齐居中 | 内存预算 | 状态 |
|:---|:---:|---:|:---|
| `core::config` | 居中文本 | 1.2 MB | 已就绪 |
| `core::storage` | 原子写回 | 0.8 MB | 已就绪 |
| `ui::reader_view` | Direct2D | 12.5 MB | 测试中 |

## 多语言围栏代码块

```rust
fn main() {
    let title = "MDReader 纯粹阅读器";
    println!("启动成功: {}", title);
}
```

```json
{
  "theme": "light",
  "fontSize": 16,
  "maxWidth": 760
}
```

## 本地图片、外部链接与 HTML 安全隔离

本地相对路径图片：

![测试图片](test-image.png)

外部链接：[参考项目 Shazao-markdown](https://github.com/yakoye/Shazao-markdown)

安全隔离测试段落（应作为纯文本或安全消毒呈现，绝不可执行脚本）：

<script>alert(1)</script>

<img src="x" onerror="alert(1)" />
