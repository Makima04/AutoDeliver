## 第一优先级：所有权与借用（系统巩固）
你虽然见过 &Vec<i32> 和 'a，但可能还没系统理解所有权的三条规则。这是 Rust 的地基，现在回头系统梳理绝对值得。
重点搞清楚：

所有权转移（move）什么时候发生
不可变引用 &T 和可变引用 &mut T 的区别，以及"同一时刻只能有一个可变引用或多个不可变引用"这条铁律
引用的作用域和原始变量的关系

很多初学者跳过了这里直接写代码，结果后面看到复杂的生命周期错误会完全懵掉。

## 第二优先级：结构体（struct）与方法（impl）
你目前还在用独立的函数操作数据。Rust 的面向对象是通过 struct + impl 实现的，这是写任何非玩具代码的必经之路。
学会定义结构体、给结构体附加方法、关联函数（比如 String::new()），以及元组结构体、单元结构体。

## 第三优先级：枚举（enum）深入与模式匹配
你用过 Option<i32>，但还没自己定义过枚举。Rust 的枚举比大多数语言强大得多，每个变体可以携带不同数据（代数数据类型）。配合 match 进行穷尽性检查，是 Rust 表达能力的核心。
学会：

自定义带数据的枚举
if let 和 while let（简化版 match）
模式匹配的 refutable/irrefutable 概念

## 第四优先级：错误处理实战
你见过了 Result，接下来要学传播错误的写法：? 运算符。这是 Rust 中处理链式错误的标准做法。
还要学 unwrap、expect 的适用场景（快速原型 vs 生产代码），以及如何用 map、and_then 等组合子处理 Result。

## 第五优先级：特质（trait）
这是 Rust 最核心的抽象机制，类似于其他语言的接口，但远比接口灵活。你要理解：

如何定义和实现 trait
trait bound（对泛型的约束）
常见的标准库 trait：Debug、Display、Clone、Copy、PartialEq、Default

只有理解了 trait，你才能真正读懂标准库文档和第三方 crate。

## 第六优先级：泛型（Generics）与 trait bound 结合
你已经见过 <T>（比如 Option<T>），但还没自己写泛型函数和结构体。结合 trait bound 使用，才能发挥 Rust 类型系统的威力。

## 第七优先级：常用集合与迭代器

HashMap<K, V>、HashSet<T>、BTreeMap 等
迭代器适配器：map、filter、collect、fold 等

Rust 中处理集合的惯用写法高度依赖迭代器，比手写 for 循环更常见。

## 第八优先级：闭包（Closure）
Rust 的闭包有独特的捕获规则（move、borrow），并且在迭代器、线程、回调中大量使用。
## 第九优先级：智能指针
Box<T>（堆分配）、Rc<T>（引用计数）、Arc<T>（原子引用计数，多线程）、RefCell<T>（内部可变性）。这些是你突破所有权限制、处理复杂数据结构的工具。
## 第十优先级：并发编程
std::thread、通道（mpsc）、Mutex、RwLock、原子类型。Rust 的并发安全是编译期保证的，理解 Send 和 Sync trait 非常重要。